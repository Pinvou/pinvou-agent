// turndown(+gfm 插件)只服务低频路径(旧 HTML 会话复制为 Markdown、
// artifacts Markdown 预览的 HTML→Markdown 回写),统一在首次用到时动态
// import,不随聊天启动链打进主 chunk;两处的构造配置必须保持一致。
export async function createTurndownService() {
  const [{ default: TurndownService }, { gfm }] = await Promise.all([
    import('turndown'),
    import('turndown-plugin-gfm'),
  ]);
  const turndown = new TurndownService({
    headingStyle: 'atx',
    bulletListMarker: '-',
    codeBlockStyle: 'fenced',
  });
  turndown.use(gfm);
  turndown.keep(['kbd']);
  return turndown;
}
