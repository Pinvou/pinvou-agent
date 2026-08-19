// 独立编译 PinvouOS 原子 Agent；共享 mod.rs 注册由集成者统一处理，避免并行改动冲突。
#[path = "../src/features/pinvou_os/capability_agent.rs"]
mod capability_agent;
#[path = "../src/features/pinvou_os/memory_agent.rs"]
mod memory_agent;
#[path = "../src/features/pinvou_os/model.rs"]
mod model;
#[path = "../src/features/pinvou_os/screen_observer_agent.rs"]
mod screen_observer_agent;
