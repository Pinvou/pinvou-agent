---
name: pinvou-browser-auth
description: Complete external browser authorization on PinvouOS Linux when QQ Music requires WeChat QR login. Use for starting, observing, or cancelling that authorization; do not use a user's claim that they scanned as proof of success.
---

# PinvouOS Browser Authorization

Use the bundled broker at `scripts/qqmusic_wechat_auth.py`. Resolve the script path relative to this `SKILL.md`; do not copy it into a session workspace.

1. Run `/usr/bin/python3 -I <skill-dir>/scripts/qqmusic_wechat_auth.py start`. It opens a dedicated headed Firefox profile. If authorization is needed, `start` waits for a visible live QR and returns `status: waiting` with `evidence.qr_ready: true`. A still-authenticated profile that previously passed the full callback flow instead returns `status: authorized` with `evidence.prior_verified: true`; do not show a scan choice in that case and continue to playback verification.
2. Only for `waiting`, immediately call `request_user_input` once using question id `qqmusic_wechat_auth_action` and exactly these options: `我已完成扫码` (Verify only), `刷新二维码` (Refresh with a new job revision), and `取消` (Cancel). Do not ask the user to speak. The browser owns the live QR challenge; do not close it or turn it into a durable artifact.
3. For Verify, run `/usr/bin/python3 -I <skill-dir>/scripts/qqmusic_wechat_auth.py status --job-id <job_id>`. Only provider evidence may return `authorized`; the user's selection or claim is never success evidence. If it is still `waiting`, keep the same pending choice surface available without claiming completion.
4. For Refresh, cancel the old job, then run `start` again and bind the choice surface to the new `job_id` revision. For Cancel or a changed target, run `/usr/bin/python3 -I <skill-dir>/scripts/qqmusic_wechat_auth.py cancel --job-id <job_id>`.

If `request_user_input` is unavailable, keep the broker in `waiting`, report `capability_unavailable`, and do not invent a Front surface or fall back to a free-text prompt. The provider may become authorized in the background, but the current Engine turn can resume only after a real card action; after `我已完成扫码`, re-read `status` instead of treating the action as evidence.

For a new flow, the broker requires current callback evidence, a new or changed authentication-cookie signal, and authenticated QQ Music UI. Warm authorization is a separate `prior_verified` fact requiring a prior three-fact marker plus a current auth cookie and current authenticated UI; it never relabels those facts as a new callback. The broker never prints QR URLs or cookie values. Do not bypass those checks, import cookies manually, attach to an unknown browser listener, or mark the parent interaction successful before the requested playback is independently verified.

The entire owned worker/browser lifecycle is hard-bounded to at most 300 seconds, including the QR wait and the post-authorization handoff window; `cancel` closes only recorded units/process groups. This broker does not start playback. After `authorized`, return the still-live browser to the original task, use an already-available control capability to request playback, and independently verify audio before claiming playback success. This workflow is Linux-only and uses a standard-library BiDi transport. If Firefox or a graphical user session is unavailable, return the broker's terminal status to Front instead of installing packages or changing proxy/VPN settings.
