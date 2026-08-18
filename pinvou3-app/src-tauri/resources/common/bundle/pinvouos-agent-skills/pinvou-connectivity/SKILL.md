---
name: pinvou-connectivity
description: Diagnose whether PinvouOS can reach the configured model endpoint and distinguish network, DNS, routing, TLS, and endpoint failures from model authorization or inference failures. Use for offline, Wi-Fi, timeout, or connectivity recovery work.
---

# PinvouOS Connectivity

Call `pinvou_runtime_status`. Read `connectivity` independently from `inference`.

1. `online` proves that the active endpoint path was reached; it does not prove the API key or model works.
2. `offline` with a reason code is network evidence. `unknown` means the route or probe itself is not configured enough to decide.
3. Compare `checkedAtMs` and latency before using a result.
4. For diagnosis, proceed from link/device facts to address and route, then DNS/TLS, then endpoint reachability. Do not change Wi-Fi, proxy, VPN, or routes without Policy approval and a reversible plan.
5. Model credentials and successful completions belong to Inference Agent, not this Agent.
