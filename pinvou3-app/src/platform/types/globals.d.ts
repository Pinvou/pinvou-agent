// Ambient declarations for tsc --checkJs over the pure-JS frontend.
// jsconfig.json picks this up for editor + `npm run check:types`.
//
// Platform globals injected by classic-script bridges before any React code
// runs (src/platform/tauri/bridge.js, src/platform/web/bridge.js). Typed as
// loosely as the JS code consumes them: unknown-shaped bridges are `any`
// because every call site predates type checking.
//
// NOTE: bare `window.X;` expression statements are NOT declarations in a
// .d.ts (they are a silent no-op); only `declare global` blocks actually
// augment the Window type.
declare global {
  interface Window {
    PinvouPlatform: any;
    TauriBridge: any;
    __TAURI__: any;
    __PINVOU_STARTUP__: any;
    __PINVOU_TAURI_BRIDGE_FEATURES__: any;
    __PINVOU_SHARED_I18N__: any;
    PinvouMarkdownRenderer: any;
    PinvouWebClient: any;
  }
}

export {};
