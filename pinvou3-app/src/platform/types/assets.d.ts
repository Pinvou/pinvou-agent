// Vite asset imports (svg/png/jpg/webp) resolve to string URLs at build time.
// Declaring them keeps tsc --checkJs from flagging every static import in
// pet-registry.js / model-catalog.js / *View.jsx as an unresolved module.
declare module '*.svg' {
  const src: string;
  export default src;
}
declare module '*.png' {
  const src: string;
  export default src;
}
declare module '*.jpg' {
  const src: string;
  export default src;
}
declare module '*.jpeg' {
  const src: string;
  export default src;
}
declare module '*.webp' {
  const src: string;
  export default src;
}
declare module '*.ico' {
  const src: string;
  export default src;
}
declare module '*.css' {
  const src: string;
  export default src;
}
