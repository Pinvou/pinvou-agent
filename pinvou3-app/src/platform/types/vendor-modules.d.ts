// turndown and turndown-plugin-gfm ship no type declarations (TS7016 in
// check:types). Declare only the minimal API surface this repo actually uses
// (src/shared/turndown-factory.js and its consumers), not full coverage.
declare module 'turndown' {
  interface TurndownOptions {
    headingStyle?: string;
    bulletListMarker?: string;
    codeBlockStyle?: string;
  }

  interface TurndownService {
    use(plugin: unknown): void;
    keep(tags: string[]): void;
    turndown(html: string): string;
  }

  const TurndownService: new (options?: TurndownOptions) => TurndownService;
  export default TurndownService;
}

declare module 'turndown-plugin-gfm' {
  export const gfm: unknown;
}
