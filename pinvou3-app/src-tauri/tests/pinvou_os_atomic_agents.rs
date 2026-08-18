//! 独立编译 PinvouOS 原子 Agent，不依赖共享 runtime 注册点。
//! 集成时只需在 features/pinvou_os/mod.rs 增加三个 mod 与 pub use。

#[path = "../src/features/pinvou_os/attention_agent.rs"]
mod attention_agent;
// model.rs 的统一账本事件包含强类型 Memory decision/checkpoint payload；原子
// Agent 仍不调用 Memory，但独立编译模型时必须把该 wire 类型提供进来。
#[path = "../src/features/pinvou_os/memory_agent.rs"]
mod memory_agent;
#[path = "../src/features/pinvou_os/model.rs"]
mod model;
#[path = "../src/features/pinvou_os/policy_agent.rs"]
mod policy_agent;
#[path = "../src/features/pinvou_os/surface_agent.rs"]
mod surface_agent;

use attention_agent::attention_allocate_contract;
use policy_agent::policy_authorize_contract;
use surface_agent::surface_observe_contract;

#[test]
fn atomic_agents_publish_unique_typed_capabilities() {
    let contracts = [
        surface_observe_contract(),
        policy_authorize_contract(),
        attention_allocate_contract(),
    ];
    let ids = contracts
        .iter()
        .map(|contract| contract.capability_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(ids.len(), 3);
    assert!(contracts.iter().all(|contract| {
        contract.version > 0
            && contract.input_schema.is_object()
            && contract.output_schema.is_object()
    }));
}
