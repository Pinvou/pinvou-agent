// FFE4-1：实现移入 shared/tool-store-events.js（平台 classic 桥接层与 feature
// 模块共用同一实现，事件名是跨层契约）；此处保留 feature 侧具名导出，消费方不变。
const { notifyComposerToolsChanged, notifyChatRoundCommitted } = window.PinvouToolStoreEvents;

export { notifyComposerToolsChanged, notifyChatRoundCommitted };
