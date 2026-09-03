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
  // ES2022 API the ES2021 lib target doesn't know. The runtime polyfill for
  // the Safari 14 floor lives in src/shared/legacy-polyfills.js (compat is
  // gated by audit-compat.mjs); declaring it here teaches the type checker
  // about the polyfilled member without raising the jsconfig lib.
  interface ObjectConstructor {
    hasOwn(object: object, propertyKey: PropertyKey): boolean;
  }

  interface Window {
    PinvouPlatform: any;
    TauriBridge: any;
    __TAURI__: any;
    __PINVOU_STARTUP__: any;
    __PINVOU_TAURI_BRIDGE_FEATURES__: any;
    __PINVOU_SHARED_I18N__: any;
    PinvouMarkdownRenderer: any;
    PinvouWebClient: any;
    // i18n string/function dictionary injected by src/app/main.jsx (uiPlatformMisc.webClientErrors),
    // read as `(window.PinvouWebClientStrings || {})[key]` in src/platform/web/bootstrap.js.
    PinvouWebClientStrings: any;
    // Safari 14 legacy constructor, used as `window.AudioContext || window.webkitAudioContext`.
    webkitAudioContext: typeof AudioContext | undefined;
  }
}

export {};
