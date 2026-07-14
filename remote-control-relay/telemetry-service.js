import crypto from "node:crypto";
import { isIP } from "node:net";
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  IPv4,
  IPv6,
  loadVectorIndexFromFile,
  newWithVectorIndex,
  verifyFromFile,
} from "ip2region.js";

const __dirname = dirname(fileURLToPath(import.meta.url));
const TELEMETRY_PREFIX = "/pinvou3/telemetry";
const STATS_PREFIX = "/pinvou3/stats";
const MAX_JSON_BYTES = 256 * 1024;
const MAX_EVENTS_PER_BATCH = 200;
const ONLINE_WINDOW_MS = 5 * 60_000;
const ADMIN_SESSION_MS = 12 * 60 * 60_000;
const DAY_MS = 24 * 60 * 60_000;
const REGION_REFRESH_MS = DAY_MS;
const STATS_TIMEZONE_OFFSET_MS = 8 * 60 * 60_000;
const DEFAULT_EVENT_RETENTION_MS = 35 * DAY_MS;
const DEFAULT_MAX_EVENTS = 100_000;
const DEFAULT_MAX_DEVICES = 10_000;
const DEFAULT_EVENT_RATE_PER_MINUTE = 2_000;
const DEFAULT_GLOBAL_EVENT_RATE_PER_MINUTE = 20_000;
const RATE_LIMIT_WINDOW_MS = 60_000;
const MAX_RATE_LIMIT_BUCKETS = 20_000;

function json(res, status, value, headers = {}) {
  res.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    "x-content-type-options": "nosniff",
    ...headers,
  });
  res.end(JSON.stringify(value));
}

function html(res, status, value) {
  res.writeHead(status, {
    "content-type": "text/html; charset=utf-8",
    "cache-control": "no-store",
    "content-security-policy": "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
    "x-content-type-options": "nosniff",
    "x-frame-options": "DENY",
    "referrer-policy": "no-referrer",
  });
  res.end(value);
}

function safeEqual(a, b) {
  const left = Buffer.from(String(a || ""));
  const right = Buffer.from(String(b || ""));
  return left.length === right.length && crypto.timingSafeEqual(left, right);
}

function randomId(prefix, bytes = 18) {
  return `${prefix}${crypto.randomBytes(bytes).toString("base64url")}`;
}

function boundedText(value, max = 128) {
  return String(value || "").trim().slice(0, max);
}

function boundedCount(value) {
  const n = Number(value);
  if (!Number.isFinite(n) || n < 0) return 0;
  return Math.min(Math.floor(n), Number.MAX_SAFE_INTEGER);
}

function boundedInteger(value, fallback, minimum, maximum) {
  const parsed = Number(value);
  if (!Number.isFinite(parsed)) return fallback;
  return Math.min(maximum, Math.max(minimum, Math.floor(parsed)));
}

class TelemetryHttpError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

function normalizedState(value) {
  return ["online", "idle", "active", "generating"].includes(value) ? value : "online";
}

function isLocalIp(value) {
  const ip = String(value || "").trim().toLowerCase();
  const version = isIP(ip);
  if (version === 4) {
    const parts = ip.split(".").map(Number);
    return parts[0] === 0
      || parts[0] === 10
      || parts[0] === 127
      || (parts[0] === 100 && parts[1] >= 64 && parts[1] <= 127)
      || (parts[0] === 169 && parts[1] === 254)
      || (parts[0] === 172 && parts[1] >= 16 && parts[1] <= 31)
      || (parts[0] === 192 && parts[1] === 168)
      || parts[0] >= 224;
  }
  if (version === 6) {
    return ip === "::" || ip === "::1" || ip.startsWith("fc") || ip.startsWith("fd")
      || /^fe[89ab]/.test(ip);
  }
  return true;
}

function regionPart(value) {
  const part = boundedText(value, 64);
  return part && part !== "0" ? part : "";
}

export function parseIp2Region(value) {
  const [country, province, city, , countryCode] = String(value || "").split("|");
  const normalized = {
    country: regionPart(country),
    province: regionPart(province),
    city: regionPart(city),
    country_code: regionPart(countryCode).toUpperCase(),
  };
  return normalized.country || normalized.province || normalized.city ? normalized : null;
}

export function formatDeviceRegion(region) {
  if (!region || typeof region !== "object") return "未知";
  const country = regionPart(region.country);
  const province = regionPart(region.province);
  const city = regionPart(region.city);
  const domestic = region.country_code === "CN" || country === "中国";
  const parts = domestic
    ? (province || city ? [province, city] : [country])
    : [country, province, city];
  const unique = parts.filter((part, index) => part && parts.indexOf(part) === index);
  return unique.slice(0, 3).join(" / ") || "未知";
}

export function createIpRegionResolver(options = {}) {
  const searchers = new Map();
  const sources = [
    [4, IPv4, options.ipv4Path],
    [6, IPv6, options.ipv6Path],
  ];
  for (const [versionNumber, version, path] of sources) {
    if (!path || !existsSync(path)) continue;
    try {
      verifyFromFile(path);
      const vectorIndex = loadVectorIndexFromFile(path);
      searchers.set(versionNumber, newWithVectorIndex(version, path, vectorIndex));
    } catch (error) {
      console.error(`[telemetry] failed to initialize IPv${versionNumber} region database:`, error);
    }
  }
  return {
    enabled: searchers.size > 0,
    async lookup(ip) {
      const version = isIP(ip);
      if (!version || isLocalIp(ip)) return null;
      const searcher = searchers.get(version);
      if (!searcher) return null;
      try {
        return parseIp2Region(await searcher.search(ip));
      } catch (error) {
        console.error(`[telemetry] failed to resolve IPv${version} region:`, error);
        return null;
      }
    },
  };
}

function statsDay(timestamp) {
  return Math.floor((Number(timestamp) + STATS_TIMEZONE_OFFSET_MS) / DAY_MS);
}

function statsDayLabel(day) {
  return new Date(day * DAY_MS).toISOString().slice(0, 10);
}

export function buildStatsOverview(devices, events, now = Date.now()) {
  const today = statsDay(now);
  const trendStart = today - 29;
  const activeWeekStart = today - 6;
  const deviceById = new Map(devices.map((device) => [device.device_id, device]));

  const online = devices.filter((device) => now - Number(device.last_seen_at || 0) <= ONLINE_WINDOW_MS);
  const activeToday = devices.filter((device) => statsDay(device.last_active_at || 0) === today);
  const activeWeek = devices.filter((device) => statsDay(device.last_active_at || 0) >= activeWeekStart);
  const trend = new Map();
  for (let day = trendStart; day <= today; day += 1) {
    trend.set(day, { day: statsDayLabel(day), turns: 0, device_ids: new Set() });
  }

  for (const event of events) {
    if (!deviceById.has(event.device_id)) continue;
    const row = trend.get(statsDay(event.occurred_at));
    if (!row) continue;
    row.turns += 1;
    row.device_ids.add(event.device_id);
  }

  const versionMap = new Map();
  for (const device of activeWeek) {
    const version = device.app_version || "未知";
    versionMap.set(version, (versionMap.get(version) || 0) + 1);
  }

  return {
    generated_at: now,
    online_window_seconds: ONLINE_WINDOW_MS / 1000,
    counts: {
      online: online.length,
      active_today: activeToday.length,
      active_7d: activeWeek.length,
    },
    usage_trend: [...trend.values()].map((row) => ({
      day: row.day,
      active_devices: row.device_ids.size,
      turns: row.turns,
    })),
    active_versions: [...versionMap.entries()]
      .map(([version, count]) => ({ version, count }))
      .sort((a, b) => b.count - a.count || a.version.localeCompare(b.version)),
  };
}

export function buildDeviceList(devices, events, now = Date.now()) {
  const weekStart = statsDay(now) - 6;
  const activity = new Map();
  for (const event of events) {
    if (statsDay(event.occurred_at) < weekStart) continue;
    const row = activity.get(event.device_id) || { turns: 0 };
    row.turns += 1;
    activity.set(event.device_id, row);
  }

  return devices
    .map((device) => {
      const recent = activity.get(device.device_id) || { turns: 0 };
      const online = now - Number(device.last_seen_at || 0) <= ONLINE_WINDOW_MS;
      return {
        device_id: device.device_id,
        app_version: device.app_version,
        platform: device.platform,
        state: device.state,
        online,
        region: formatDeviceRegion(device.region),
        first_seen_at: device.first_seen_at,
        last_active_at: device.last_active_at,
        turns_7d: recent.turns,
      };
    })
    .sort((a, b) => Number(b.online) - Number(a.online)
      || Number(b.last_active_at || 0) - Number(a.last_active_at || 0));
}

function readCookies(req) {
  const values = {};
  for (const part of String(req.headers.cookie || "").split(";")) {
    const index = part.indexOf("=");
    if (index <= 0) continue;
    values[part.slice(0, index).trim()] = decodeURIComponent(part.slice(index + 1).trim());
  }
  return values;
}

async function readJson(req) {
  const chunks = [];
  let size = 0;
  for await (const chunk of req) {
    size += chunk.length;
    if (size > MAX_JSON_BYTES) throw new Error("payload_too_large");
    chunks.push(chunk);
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8") || "{}");
  } catch {
    throw new Error("invalid_json");
  }
}

function loadJson(path, fallback) {
  if (!existsSync(path)) return fallback;
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    console.error(`[telemetry] failed to read ${path}:`, error);
    return fallback;
  }
}

function atomicWriteJson(path, value) {
  const temporary = `${path}.tmp`;
  writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
  renameSync(temporary, path);
}

function atomicWriteJsonLines(path, values) {
  const temporary = `${path}.tmp`;
  const content = values.length
    ? `${values.map((value) => JSON.stringify(value)).join("\n")}\n`
    : "";
  writeFileSync(temporary, content, { mode: 0o600 });
  renameSync(temporary, path);
}

export class TelemetryStore {
  constructor(dataDir, pepper, options = {}) {
    this.dataDir = dataDir;
    this.pepper = pepper;
    this.devicesPath = join(dataDir, "devices.json");
    this.eventsPath = join(dataDir, "usage-events.jsonl");
    mkdirSync(dataDir, { recursive: true, mode: 0o700 });
    this.devices = new Map();
    this.devicesByHardwareHash = new Map();
    this.events = [];
    this.eventIds = new Set();
    this.saveTimer = null;
    this.lastEventCompactionAt = 0;
    this.eventRateBuckets = new Map();
    this.globalEventRateBucket = null;
    this.maxDevices = boundedInteger(options.maxDevices, DEFAULT_MAX_DEVICES, 1, 1_000_000);
    this.maxEvents = boundedInteger(options.maxEvents, DEFAULT_MAX_EVENTS, 100, 5_000_000);
    this.eventCompactionTarget = Math.max(1, Math.floor(this.maxEvents * 0.9));
    this.eventRetentionMs = boundedInteger(
      options.eventRetentionMs,
      DEFAULT_EVENT_RETENTION_MS,
      30 * DAY_MS,
      366 * DAY_MS,
    );
    this.eventRatePerMinute = boundedInteger(
      options.eventRatePerMinute,
      DEFAULT_EVENT_RATE_PER_MINUTE,
      MAX_EVENTS_PER_BATCH,
      100_000,
    );
    this.globalEventRatePerMinute = boundedInteger(
      options.globalEventRatePerMinute,
      DEFAULT_GLOBAL_EVENT_RATE_PER_MINUTE,
      MAX_EVENTS_PER_BATCH,
      1_000_000,
    );

    const saved = loadJson(this.devicesPath, { devices: [] });
    for (const device of Array.isArray(saved.devices) ? saved.devices : []) {
      if (!device?.device_id || !device?.hardware_hash || !device?.device_token) continue;
      const normalized = {
        ...device,
        turns: 0,
        input_tokens: 0,
        output_tokens: 0,
        errors: 0,
      };
      this.devices.set(device.device_id, normalized);
      this.devicesByHardwareHash.set(device.hardware_hash, normalized);
    }
    this.loadEvents();
  }

  loadEvents() {
    if (!existsSync(this.eventsPath)) return;
    const lines = readFileSync(this.eventsPath, "utf8").split("\n");
    let rewriteNeeded = false;
    for (const line of lines) {
      if (!line.trim()) continue;
      try {
        const event = JSON.parse(line);
        if (!event.event_id || this.eventIds.has(event.event_id)) {
          rewriteNeeded = true;
          continue;
        }
        const device = this.devices.get(event.device_id);
        if (!device) {
          rewriteNeeded = true;
          continue;
        }
        this.eventIds.add(event.event_id);
        this.events.push(event);
      } catch (error) {
        rewriteNeeded = true;
        console.error("[telemetry] skipped malformed usage event:", error);
      }
    }
    this.compactEvents(Date.now(), rewriteNeeded);
    this.rebuildUsageCounters();
  }

  hardwareHash(claim) {
    return crypto.createHmac("sha256", this.pepper).update(claim).digest("hex");
  }

  registrationSecretHash(secret) {
    return crypto.createHmac("sha256", this.pepper)
      .update(`registration-secret:${secret}`)
      .digest("hex");
  }

  saveSoon() {
    if (this.saveTimer) return;
    this.saveTimer = setTimeout(() => {
      this.saveTimer = null;
      this.saveNow();
    }, 1000);
    this.saveTimer.unref?.();
  }

  saveNow() {
    atomicWriteJson(this.devicesPath, { version: 2, devices: [...this.devices.values()] });
  }

  updateRegion(device, region, now = Date.now()) {
    if (region !== undefined) {
      if (region) device.region = region;
      device.region_updated_at = now;
    }
  }

  regionNeedsRefresh(device, now = Date.now()) {
    return now - Number(device.region_updated_at || 0) >= REGION_REFRESH_MS;
  }

  register(input, region) {
    const claim = boundedText(input.hardware_claim, 512);
    if (claim.length < 8) throw new Error("invalid_hardware_claim");
    const registrationSecret = boundedText(input.registration_secret, 256);
    if (registrationSecret.length < 32) throw new Error("invalid_registration_secret");
    const hardwareHash = this.hardwareHash(claim);
    const registrationSecretHash = this.registrationSecretHash(registrationSecret);
    let device = this.devicesByHardwareHash.get(hardwareHash);
    const now = Date.now();
    if (!device) {
      if (this.devices.size >= this.maxDevices) {
        throw new TelemetryHttpError(503, "device_capacity_reached");
      }
      device = {
        device_id: randomId("dev_", 15),
        device_token: randomId("dt_", 24),
        hardware_hash: hardwareHash,
        registration_secret_hash: registrationSecretHash,
        identity_quality: boundedText(input.identity_quality, 32) || "installation_only",
        hardware_source: boundedText(input.hardware_source, 48) || "unknown",
        first_seen_at: now,
        last_seen_at: now,
        last_active_at: null,
        app_version: boundedText(input.app_version, 32),
        platform: boundedText(input.platform, 32),
        arch: boundedText(input.arch, 32),
        state: "online",
        region: region || null,
        region_updated_at: now,
        turns: 0,
        input_tokens: 0,
        output_tokens: 0,
        errors: 0,
      };
      this.devices.set(device.device_id, device);
      this.devicesByHardwareHash.set(device.hardware_hash, device);
      this.saveNow();
    } else {
      if (!device.registration_secret_hash
        || !safeEqual(registrationSecretHash, device.registration_secret_hash)) {
        throw new TelemetryHttpError(409, "device_already_registered");
      }
      device.last_seen_at = now;
      device.app_version = boundedText(input.app_version, 32) || device.app_version;
      device.platform = boundedText(input.platform, 32) || device.platform;
      device.arch = boundedText(input.arch, 32) || device.arch;
      this.updateRegion(device, region, now);
      this.saveSoon();
    }
    return { device_id: device.device_id, device_token: device.device_token };
  }

  authenticate(deviceId, token) {
    const device = this.devices.get(boundedText(deviceId, 96));
    if (!device || !safeEqual(token, device.device_token)) return null;
    return device;
  }

  heartbeat(device, input, region) {
    const now = Date.now();
    device.last_seen_at = now;
    device.state = normalizedState(input.state);
    device.app_version = boundedText(input.app_version, 32) || device.app_version;
    device.platform = boundedText(input.platform, 32) || device.platform;
    device.arch = boundedText(input.arch, 32) || device.arch;
    const activity = Number(input.last_activity_at);
    if (Number.isFinite(activity) && activity > 0 && activity <= now + 60_000) {
      device.last_active_at = Math.max(Number(device.last_active_at || 0), activity);
    }
    this.updateRegion(device, region, now);
    this.bindRegistrationSecret(device, input.registration_secret);
    this.saveSoon();
  }

  bindRegistrationSecret(device, secret) {
    if (device.registration_secret_hash) return;
    const registrationSecret = boundedText(secret, 256);
    if (registrationSecret.length < 32) return;
    device.registration_secret_hash = this.registrationSecretHash(registrationSecret);
  }

  applyUsage(device, event) {
    device.turns += 1;
    device.input_tokens += event.input_tokens;
    device.output_tokens += event.output_tokens;
    if (!event.success) device.errors += 1;
    device.last_active_at = Math.max(Number(device.last_active_at || 0), event.occurred_at);
  }

  rebuildUsageCounters() {
    for (const device of this.devices.values()) {
      device.turns = 0;
      device.input_tokens = 0;
      device.output_tokens = 0;
      device.errors = 0;
    }
    for (const event of this.events) {
      const device = this.devices.get(event.device_id);
      if (device) this.applyUsage(device, event);
    }
  }

  compactEvents(now = Date.now(), force = false) {
    const cutoff = now - this.eventRetentionMs;
    let retained = this.events.filter((event) => Number(event.received_at || event.occurred_at) >= cutoff);
    if (retained.length > this.maxEvents) retained = retained.slice(-this.eventCompactionTarget);
    const changed = force || retained.length !== this.events.length;
    this.events = retained;
    this.eventIds = new Set(retained.map((event) => event.event_id));
    this.lastEventCompactionAt = now;
    if (changed) atomicWriteJsonLines(this.eventsPath, retained);
    return changed;
  }

  compactExpiredEventsIfDue(now = Date.now()) {
    if (now - this.lastEventCompactionAt < 60 * 60_000) return false;
    return this.compactEvents(now);
  }

  eventRateLimited(deviceId, count, now = Date.now()) {
    const bucket = this.eventRateBuckets.get(deviceId);
    let deviceLimited;
    if (!bucket || now - bucket.started_at >= RATE_LIMIT_WINDOW_MS) {
      this.eventRateBuckets.set(deviceId, { started_at: now, count });
      deviceLimited = count > this.eventRatePerMinute;
    } else {
      bucket.count += count;
      deviceLimited = bucket.count > this.eventRatePerMinute;
    }
    if (deviceLimited) return true;
    if (!this.globalEventRateBucket
      || now - this.globalEventRateBucket.started_at >= RATE_LIMIT_WINDOW_MS) {
      this.globalEventRateBucket = { started_at: now, count };
    } else {
      this.globalEventRateBucket.count += count;
    }
    return this.globalEventRateBucket.count > this.globalEventRatePerMinute;
  }

  appendEvents(device, inputEvents) {
    const accepted = [];
    const duplicates = [];
    const now = Date.now();
    if (this.compactExpiredEventsIfDue(now)) this.rebuildUsageCounters();
    if (this.eventRateLimited(device.device_id, inputEvents.length, now)) {
      throw new TelemetryHttpError(429, "event_rate_limited");
    }
    const batchEventIds = new Set();
    for (const raw of inputEvents.slice(0, MAX_EVENTS_PER_BATCH)) {
      const eventId = boundedText(raw?.event_id, 96);
      if (eventId.length < 12) continue;
      if (this.eventIds.has(eventId) || batchEventIds.has(eventId)) {
        duplicates.push(eventId);
        continue;
      }
      batchEventIds.add(eventId);
      let occurredAt = Number(raw.occurred_at);
      if (!Number.isFinite(occurredAt) || occurredAt <= 0 || occurredAt > now + 60_000) occurredAt = now;
      const event = {
        event_id: eventId,
        device_id: device.device_id,
        occurred_at: occurredAt,
        received_at: now,
        input_tokens: boundedCount(raw.input_tokens),
        output_tokens: boundedCount(raw.output_tokens),
        success: raw.success !== false,
      };
      accepted.push(event);
    }
    if (accepted.length) {
      appendFileSync(this.eventsPath, accepted.map((event) => JSON.stringify(event)).join("\n") + "\n", { mode: 0o600 });
      for (const event of accepted) {
        this.eventIds.add(event.event_id);
        this.events.push(event);
        this.applyUsage(device, event);
      }
      if (this.events.length > this.maxEvents) {
        this.compactEvents(now, true);
        this.rebuildUsageCounters();
      }
      device.last_seen_at = now;
      this.saveSoon();
    }
    return { accepted: accepted.map((event) => event.event_id), duplicates };
  }

  overview() {
    if (this.compactExpiredEventsIfDue()) this.rebuildUsageCounters();
    return buildStatsOverview([...this.devices.values()], this.events);
  }

  deviceList() {
    if (this.compactExpiredEventsIfDue()) this.rebuildUsageCounters();
    return buildDeviceList([...this.devices.values()], this.events);
  }
}

export function createTelemetryService(options = {}) {
  const enrollmentToken = String(process.env.PINVOU_TELEMETRY_ENROLLMENT_TOKEN || "");
  const devicePepper = String(process.env.PINVOU_TELEMETRY_DEVICE_PEPPER || "");
  const adminPassword = String(process.env.PINVOU_STATS_ADMIN_PASSWORD || "");
  const dataDir = process.env.PINVOU_TELEMETRY_DATA_DIR || "/var/lib/pinvou-telemetry";
  const enabled = enrollmentToken.length >= 24 && devicePepper.length >= 24 && adminPassword.length >= 12;
  const store = enabled ? new TelemetryStore(dataDir, devicePepper, {
    maxDevices: options.maxDevices ?? process.env.PINVOU_TELEMETRY_MAX_DEVICES,
    maxEvents: options.maxEvents ?? process.env.PINVOU_TELEMETRY_MAX_EVENTS,
    eventRetentionMs: options.eventRetentionMs
      ?? (Number(process.env.PINVOU_TELEMETRY_EVENT_RETENTION_DAYS) * DAY_MS || undefined),
    eventRatePerMinute: options.eventRatePerMinute
      ?? process.env.PINVOU_TELEMETRY_EVENT_RATE_PER_MINUTE,
    globalEventRatePerMinute: options.globalEventRatePerMinute
      ?? process.env.PINVOU_TELEMETRY_EVENT_RATE_GLOBAL_PER_MINUTE,
  }) : null;
  const regionResolver = options.regionResolver || createIpRegionResolver({
    ipv4Path: process.env.PINVOU_TELEMETRY_IP_DB_V4 || join(dataDir, "ip2region_v4.xdb"),
    ipv6Path: process.env.PINVOU_TELEMETRY_IP_DB_V6 || join(dataDir, "ip2region_v6.xdb"),
  });
  const statsPage = options.statsPage || join(__dirname, "web", "stats.html");
  const adminSessions = new Map();
  const loginAttempts = new Map();
  const registrationAttempts = new Map();
  const globalRegistrationAttempts = new Map();
  const registrationRatePerIp = boundedInteger(
    options.registrationRatePerIp ?? process.env.PINVOU_TELEMETRY_REGISTER_RATE_PER_IP,
    10,
    1,
    1_000,
  );
  const registrationRateGlobal = boundedInteger(
    options.registrationRateGlobal ?? process.env.PINVOU_TELEMETRY_REGISTER_RATE_GLOBAL,
    100,
    1,
    100_000,
  );

  function bearer(req) {
    const value = String(req.headers.authorization || "");
    return value.startsWith("Bearer ") ? value.slice(7) : "";
  }

  function adminAuthorized(req) {
    const token = readCookies(req).pinvou_stats_session;
    const expiresAt = token ? adminSessions.get(token) : null;
    if (!expiresAt || expiresAt <= Date.now()) {
      if (token) adminSessions.delete(token);
      return false;
    }
    return true;
  }

  function rateLimited(buckets, key, limit) {
    const now = Date.now();
    const bucket = buckets.get(key);
    if (!bucket || now - bucket.started_at >= 60_000) {
      if (!bucket && buckets.size >= MAX_RATE_LIMIT_BUCKETS) {
        for (const [bucketKey, value] of buckets) {
          if (now - value.started_at >= RATE_LIMIT_WINDOW_MS) buckets.delete(bucketKey);
        }
        while (buckets.size >= MAX_RATE_LIMIT_BUCKETS) {
          buckets.delete(buckets.keys().next().value);
        }
      }
      buckets.set(key, { started_at: now, count: 1 });
      return false;
    }
    bucket.count += 1;
    return bucket.count > limit;
  }

  async function handleHttp(req, res, routePath, context = {}) {
    const isTelemetry = routePath === TELEMETRY_PREFIX || routePath.startsWith(`${TELEMETRY_PREFIX}/`);
    const isStats = routePath === STATS_PREFIX || routePath.startsWith(`${STATS_PREFIX}/`);
    if (!isTelemetry && !isStats) return false;
    if (!enabled) {
      json(res, 503, { error: "telemetry_not_configured" });
      return true;
    }

    try {
      const requestIp = boundedText(context.clientIp || req.socket?.remoteAddress || "unknown", 96);
      if (req.method === "GET" && routePath === `${TELEMETRY_PREFIX}/healthz`) {
        json(res, 200, { ok: true });
        return true;
      }
      if (req.method === "POST" && routePath === `${TELEMETRY_PREFIX}/v1/register`) {
        if (rateLimited(registrationAttempts, requestIp, registrationRatePerIp)
          || rateLimited(globalRegistrationAttempts, "global", registrationRateGlobal)) {
          json(res, 429, { error: "rate_limited" });
          return true;
        }
        const input = await readJson(req);
        if (!safeEqual(input.enrollment_token, enrollmentToken)) {
          json(res, 401, { error: "invalid_enrollment_token" });
          return true;
        }
        const region = await regionResolver.lookup(requestIp);
        json(res, 200, store.register(input, region));
        return true;
      }
      if (req.method === "POST" && routePath === `${TELEMETRY_PREFIX}/v1/heartbeat`) {
        const input = await readJson(req);
        const device = store.authenticate(input.device_id, bearer(req));
        if (!device) {
          json(res, 401, { error: "invalid_device_token" });
          return true;
        }
        const region = store.regionNeedsRefresh(device)
          ? await regionResolver.lookup(requestIp)
          : undefined;
        store.heartbeat(device, input, region);
        json(res, 200, { ok: true, server_time: Date.now() });
        return true;
      }
      if (req.method === "POST" && routePath === `${TELEMETRY_PREFIX}/v1/events`) {
        const input = await readJson(req);
        const device = store.authenticate(input.device_id, bearer(req));
        if (!device) {
          json(res, 401, { error: "invalid_device_token" });
          return true;
        }
        const events = Array.isArray(input.events) ? input.events : [];
        if (events.length > MAX_EVENTS_PER_BATCH) {
          json(res, 413, { error: "too_many_events" });
          return true;
        }
        json(res, 200, store.appendEvents(device, events));
        return true;
      }
      if (req.method === "GET" && (routePath === STATS_PREFIX || routePath === `${STATS_PREFIX}/`)) {
        html(res, 200, readFileSync(statsPage, "utf8"));
        return true;
      }
      if (req.method === "POST" && routePath === `${STATS_PREFIX}/api/login`) {
        if (rateLimited(loginAttempts, requestIp, 30)) {
          json(res, 429, { error: "rate_limited" });
          return true;
        }
        const input = await readJson(req);
        if (!safeEqual(input.password, adminPassword)) {
          json(res, 401, { error: "invalid_credentials" });
          return true;
        }
        const session = randomId("as_", 32);
        adminSessions.set(session, Date.now() + ADMIN_SESSION_MS);
        json(res, 200, { ok: true }, {
          "set-cookie": `pinvou_stats_session=${encodeURIComponent(session)}; Path=${STATS_PREFIX}; Max-Age=${ADMIN_SESSION_MS / 1000}; HttpOnly; Secure; SameSite=Strict`,
        });
        return true;
      }
      if (req.method === "POST" && routePath === `${STATS_PREFIX}/api/logout`) {
        const session = readCookies(req).pinvou_stats_session;
        if (session) adminSessions.delete(session);
        json(res, 200, { ok: true }, {
          "set-cookie": `pinvou_stats_session=; Path=${STATS_PREFIX}; Max-Age=0; HttpOnly; Secure; SameSite=Strict`,
        });
        return true;
      }
      if (routePath.startsWith(`${STATS_PREFIX}/api/`) && !adminAuthorized(req)) {
        json(res, 401, { error: "unauthorized" });
        return true;
      }
      if (req.method === "GET" && routePath === `${STATS_PREFIX}/api/overview`) {
        json(res, 200, store.overview());
        return true;
      }
      if (req.method === "GET" && routePath === `${STATS_PREFIX}/api/devices`) {
        json(res, 200, { devices: store.deviceList() });
        return true;
      }
      json(res, 404, { error: "not_found" });
      return true;
    } catch (error) {
      const code = Number(error?.status)
        || (error?.message === "payload_too_large" ? 413 : 400);
      if (!(error instanceof TelemetryHttpError)) {
        console.error("[telemetry] request failed:", error);
      }
      json(res, code, { error: error?.message || "bad_request" });
      return true;
    }
  }

  if (!enabled) {
    console.warn("[telemetry] disabled: configure enrollment token, device pepper and admin password");
  } else {
    console.log(`[telemetry] ready data_dir=${dataDir} region_lookup=${regionResolver.enabled ? "enabled" : "disabled"}`);
  }
  return { enabled, handleHttp };
}
