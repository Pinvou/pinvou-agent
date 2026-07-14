import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  TelemetryStore,
  buildDeviceList,
  buildStatsOverview,
  formatDeviceRegion,
  parseIp2Region,
} from "../telemetry-service.js";

const at = (value) => Date.parse(`${value}+08:00`);

test("dashboard metrics use unique devices and 30 complete Beijing calendar days", () => {
  const now = at("2026-07-14T12:00:00");
  const devices = [
    {
      device_id: "dev_a",
      first_seen_at: at("2026-07-06T10:00:00"),
      last_seen_at: now - 10_000,
      last_active_at: at("2026-07-14T11:00:00"),
      app_version: "1.0.0",
      platform: "linux",
      state: "online",
      region: {
        country: "中国",
        province: "广东省",
        city: "深圳市",
        country_code: "CN",
      },
    },
    {
      device_id: "dev_b",
      first_seen_at: at("2026-07-13T09:00:00"),
      last_seen_at: now - 360_000,
      last_active_at: at("2026-07-14T10:00:00"),
      app_version: "2.0.0",
      platform: "windows",
      state: "online",
    },
    {
      device_id: "dev_c",
      first_seen_at: at("2026-07-14T08:00:00"),
      last_seen_at: now - 360_000,
      last_active_at: at("2026-07-14T09:00:00"),
      app_version: "1.0.0",
      platform: "linux",
      state: "online",
    },
  ];
  const events = [
    { event_id: "evt_a_d1", device_id: "dev_a", occurred_at: at("2026-07-07T10:00:00"), success: true },
    { event_id: "evt_a_d7", device_id: "dev_a", occurred_at: at("2026-07-13T10:00:00"), success: true },
    { event_id: "evt_a_today", device_id: "dev_a", occurred_at: at("2026-07-14T11:00:00"), success: false },
    { event_id: "evt_b_today", device_id: "dev_b", occurred_at: at("2026-07-14T10:00:00"), success: true },
    { event_id: "evt_c_today", device_id: "dev_c", occurred_at: at("2026-07-14T09:00:00"), success: true },
  ];

  const overview = buildStatsOverview(devices, events, now);
  assert.equal(overview.online_window_seconds, 300);
  assert.deepEqual(overview.counts, { online: 1, active_today: 3, active_7d: 3 });
  assert.deepEqual(overview.active_versions, [
    { version: "1.0.0", count: 2 },
    { version: "2.0.0", count: 1 },
  ]);
  assert.equal(overview.usage_trend.length, 30);
  assert.deepEqual(overview.usage_trend.at(-2), {
    day: "2026-07-13",
    active_devices: 1,
    turns: 1,
  });
  assert.deepEqual(overview.usage_trend.at(-1), {
    day: "2026-07-14",
    active_devices: 3,
    turns: 3,
  });

  const list = buildDeviceList(devices, events, now);
  assert.equal(list[0].device_id, "dev_a");
  assert.equal(list[0].first_seen_at, at("2026-07-06T10:00:00"));
  assert.equal(list[0].region, "广东省 / 深圳市");
  assert.equal(list[0].turns_7d, 2);
  assert.equal(list[1].turns_7d, 1);
  assert.equal(list[1].region, "未知");
  assert.equal("failure_rate_7d" in list[0], false);
});

test("offline IP region data keeps only coarse location fields", () => {
  const region = parseIp2Region("中国|广东省|深圳市|电信|CN");
  assert.deepEqual(region, {
    country: "中国",
    province: "广东省",
    city: "深圳市",
    country_code: "CN",
  });
  assert.equal(formatDeviceRegion(region), "广东省 / 深圳市");
  assert.equal(formatDeviceRegion(parseIp2Region("Singapore|0|Singapore|0|SG")), "Singapore");
  assert.equal(parseIp2Region(""), null);
});

test("registration requires the installation secret and enforces device capacity", () => {
  const dataDir = mkdtempSync(join(tmpdir(), "pinvou-telemetry-store-"));
  try {
    const store = new TelemetryStore(dataDir, "test-device-pepper-at-least-24-chars", {
      maxDevices: 2,
    });
    const registration = {
      hardware_claim: "hardware-claim-0001",
      registration_secret: "registration-secret-000000000000000001",
      hardware_source: "test",
      identity_quality: "hardware_serial",
      app_version: "0.5.10",
      platform: "linux",
      arch: "aarch64",
    };
    const first = store.register(registration, null);
    assert.deepEqual(store.register(registration, null), first);
    assert.throws(
      () => store.register({
        ...registration,
        registration_secret: "different-secret-0000000000000000001",
      }, null),
      (error) => error.status === 409 && error.message === "device_already_registered",
    );

    const legacyDevice = store.authenticate(first.device_id, first.device_token);
    delete legacyDevice.registration_secret_hash;
    assert.throws(
      () => store.register(registration, null),
      (error) => error.status === 409 && error.message === "device_already_registered",
    );
    store.heartbeat(legacyDevice, {
      registration_secret: registration.registration_secret,
      state: "online",
    });
    assert.deepEqual(store.register(registration, null), first);

    store.register({
      ...registration,
      hardware_claim: "hardware-claim-0002",
      registration_secret: "registration-secret-000000000000000002",
    }, null);
    assert.throws(
      () => store.register({
        ...registration,
        hardware_claim: "hardware-claim-0003",
        registration_secret: "registration-secret-000000000000000003",
      }, null),
      (error) => error.status === 503 && error.message === "device_capacity_reached",
    );
  } finally {
    rmSync(dataDir, { recursive: true, force: true });
  }
});

test("event store keeps a bounded recent window on disk and after restart", () => {
  const dataDir = mkdtempSync(join(tmpdir(), "pinvou-telemetry-events-"));
  try {
    const options = {
      maxEvents: 100,
      eventRatePerMinute: 1_000,
      globalEventRatePerMinute: 200,
    };
    const store = new TelemetryStore(
      dataDir,
      "test-device-pepper-at-least-24-chars",
      options,
    );
    const registration = {
      hardware_claim: "bounded-events-hardware",
      registration_secret: "registration-secret-bounded-events-00001",
      hardware_source: "test",
      identity_quality: "hardware_serial",
      app_version: "0.5.10",
      platform: "linux",
      arch: "aarch64",
    };
    const credential = store.register(registration, null);
    const device = store.authenticate(credential.device_id, credential.device_token);
    const now = Date.now();
    const events = Array.from({ length: 120 }, (_, index) => ({
      event_id: `evt_bounded_${String(index).padStart(6, "0")}`,
      occurred_at: now + index,
      input_tokens: 1,
      output_tokens: 1,
      success: true,
    }));
    assert.equal(store.appendEvents(device, events).accepted.length, 120);
    assert.equal(store.events.length, 90);
    assert.equal(readFileSync(join(dataDir, "usage-events.jsonl"), "utf8").trim().split("\n").length, 90);
    assert.throws(
      () => store.appendEvents(device, events.slice(0, 100)),
      (error) => error.status === 429 && error.message === "event_rate_limited",
    );

    const restarted = new TelemetryStore(
      dataDir,
      "test-device-pepper-at-least-24-chars",
      options,
    );
    assert.equal(restarted.events.length, 90);
    assert.equal(restarted.eventIds.size, 90);
  } finally {
    rmSync(dataDir, { recursive: true, force: true });
  }
});
