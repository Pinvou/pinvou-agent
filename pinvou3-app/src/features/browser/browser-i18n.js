// 浏览器 MCP server 文案（zh/en/ja 三语补丁，模块加载即挂载到 dict）。
import { dict } from '../../shared/i18n.js';

Object.assign(dict.zh, {
  browser: '浏览器',
  browserNewTab: '新标签页',
  browserUrlPlaceholder: '输入网址，回车打开',
  browserBack: '后退',
  browserForward: '前进',
  browserRefresh: '刷新',
  browserHome: '主页',
  browserOpenExternal: '在系统浏览器打开',
  browserStop: '关闭浏览器',
  browserLoading: '连接浏览器…',
  browserNotRunning: '浏览器未启动。工作模式中让 Agent 使用浏览器能力后会自动出现。',
  browserError: '浏览器操作失败',
  browserTabClose: '关闭标签页',
  browserEmptyTab: '新标签页',
});

Object.assign(dict.en, {
  browser: 'Browser',
  browserNewTab: 'New tab',
  browserUrlPlaceholder: 'Enter URL, press Enter',
  browserBack: 'Back',
  browserForward: 'Forward',
  browserRefresh: 'Refresh',
  browserHome: 'Home',
  browserOpenExternal: 'Open in system browser',
  browserStop: 'Close browser',
  browserLoading: 'Connecting to browser…',
  browserNotRunning:
    'Browser is not running. It appears automatically once the Agent uses browser capabilities in work mode.',
  browserError: 'Browser operation failed',
  browserTabClose: 'Close tab',
  browserEmptyTab: 'New tab',
});

Object.assign(dict.ja, {
  browser: 'ブラウザ',
  browserNewTab: '新しいタブ',
  browserUrlPlaceholder: 'URLを入力して Enter',
  browserBack: '戻る',
  browserForward: '進む',
  browserRefresh: '更新',
  browserHome: 'ホーム',
  browserOpenExternal: 'システムブラウザで開く',
  browserStop: 'ブラウザを閉じる',
  browserLoading: 'ブラウザに接続中…',
  browserNotRunning:
    'ブラウザは起動していません。ワークモードでエージェントがブラウザ機能を使うと自動的に表示されます。',
  browserError: 'ブラウザ操作に失敗しました',
  browserTabClose: 'タブを閉じる',
  browserEmptyTab: '新しいタブ',
});
