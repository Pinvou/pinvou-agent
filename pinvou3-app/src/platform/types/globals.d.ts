// Ambient declarations for tsc --checkJs over the pure-JS frontend.
// jsconfig.json picks this up for editor + `npm run check:types`.
//
// Platform globals injected by classic-script bridges before any React code
// runs (src/platform/tauri/bridge.js, src/platform/web/bridge.js). Typed as
// loosely as the JS code consumes them: unknown-shaped bridges are `any`
// because every call site predates type checking.
window.PinvouPlatform;
window.TauriBridge;
window.__TAURI__;
window.__PINVOU_STARTUP__;
window.__PINVOU_TAURI_BRIDGE_FEATURES__;
window.__PINVOU_SHARED_I18N__;
window.__PINVOU_MARKDOWN_BRIDGE__;
window.PinvouMarkdownRenderer;
window.PinvouWebClient;
window.__PINVOU_PET_BRIDGE__;
