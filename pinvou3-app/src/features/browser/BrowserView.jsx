// 浏览器视图（工作模式浏览器 Tab 主体）：
// - 显示专用有头 Chrome 的 CDP 截图流（browser:frame 事件 → <img>，rAF 节流）
// - 用户交互转发：点击/滚轮/键盘 → browser_input → CDP Input 域
// - 地址栏导航 + 后退/前进/刷新 + 多标签页 + 在系统浏览器打开
// 仅当工作模式中模型实际调用过浏览器能力（Rust 端 emit browser:activated）后挂载，
// 未调用时不渲染、不加载。

import { useCallback, useEffect, useRef, useState } from 'react';
import '../../features/browser/browser-i18n.js'; // 三语文案补丁（side effect）
import { invokeTauri, listenTauri } from '../../platform/tauri/client.js';
import {
  AppWindow,
  ChevronLeft,
  ExternalLink,
  Globe,
  Maximize2,
  Plus,
  RefreshCw,
  XIcon,
} from '../../components/icons.jsx';

const HOME_URL = 'https://www.bing.com';

export function BrowserView({ theme, t }) {
  const isDark = theme === 'dark';
  const [frameData, setFrameData] = useState(null); // base64 jpeg
  const [frameMeta, setFrameMeta] = useState(null); // viewport metadata
  const [url, setUrl] = useState('');
  const [urlInput, setUrlInput] = useState('');
  const [tabs, setTabs] = useState([]);
  const [activeSession, setActiveSession] = useState(null);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState('');
  const imgRef = useRef(null);
  const pendingFrame = useRef(null);
  const rafId = useRef(0);
  const activeSessionRef = useRef(null);
  const lastMove = useRef(0);

  // ---- 状态同步 ----
  const refreshStatus = useCallback(async () => {
    try {
      const st = await invokeTauri('browser_status');
      setRunning(!!st.running);
      if (st.url) setUrl(st.url);
      if (st.activeTab) setActiveSession(st.activeTab);
    } catch {
      setRunning(false);
    }
  }, []);

  const refreshTabs = useCallback(async () => {
    try {
      const list = await invokeTauri('browser_list_tabs');
      setTabs(list || []);
    } catch {
      /* 浏览器未就绪时静默 */
    }
  }, []);

  useEffect(() => {
    refreshStatus();
    refreshTabs();
    const unsubs = [];
    listenTauri('browser:frame', (e) => {
      // rAF 节流：只渲染最新帧
      pendingFrame.current = e.payload;
      if (!rafId.current) {
        rafId.current = requestAnimationFrame(() => {
          const p = pendingFrame.current;
          pendingFrame.current = null;
          rafId.current = 0;
          if (p) {
            setFrameData(p.data || null);
            setFrameMeta(p.metadata || null);
          }
        });
      }
    }).then((u) => unsubs.push(u));
    listenTauri('browser:navigation', (e) => {
      if (e.payload && e.payload.url) {
        setUrl(e.payload.url);
        setUrlInput(e.payload.url);
      }
    }).then((u) => unsubs.push(u));
    listenTauri('browser:tabs-changed', () => refreshTabs()).then((u) => unsubs.push(u));
    listenTauri('browser:activated', () => {
      refreshStatus();
      refreshTabs();
    }).then((u) => unsubs.push(u));
    listenTauri('browser:stopped', () => {
      setRunning(false);
      setFrameData(null);
    }).then((u) => unsubs.push(u));
    return () => {
      unsubs.forEach((u) => u && u());
      if (rafId.current) cancelAnimationFrame(rafId.current);
    };
  }, [refreshStatus, refreshTabs]);

  // ---- 导航 ----
  const navigate = useCallback(async (raw) => {
    let target = (raw || '').trim();
    if (!target) return;
    if (!/^https?:\/\//i.test(target) && target !== 'about:blank') {
      target = 'https://' + target;
    }
    try {
      setError('');
      await invokeTauri('browser_navigate', { url: target });
      setUrlInput(target);
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    }
  }, []);

  const runNav = useCallback(async (cmd) => {
    try {
      setError('');
      await invokeTauri(cmd);
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    }
  }, []);

  const openExternal = useCallback(async () => {
    if (!url || url === 'about:blank') return;
    try {
      await invokeTauri('open_user_external_url', { url });
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    }
  }, [url]);

  // ---- 多标签 ----
  const createTab = useCallback(async () => {
    try {
      await invokeTauri('browser_create_tab', { url: HOME_URL });
      refreshTabs();
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    }
  }, [refreshTabs]);

  const closeTab = useCallback(
    async (targetId) => {
      try {
        await invokeTauri('browser_close_tab', { targetId });
        refreshTabs();
        refreshStatus();
      } catch (e) {
        setError(typeof e === 'string' ? e : String(e));
      }
    },
    [refreshTabs, refreshStatus]
  );

  const activateTab = useCallback(
    async (sessionId) => {
      try {
        await invokeTauri('browser_activate_tab', { sessionId });
        activeSessionRef.current = sessionId;
        setActiveSession(sessionId);
      } catch (e) {
        setError(typeof e === 'string' ? e : String(e));
      }
    },
    []
  );

  const stopBrowser = useCallback(async () => {
    try {
      await invokeTauri('browser_stop');
      setRunning(false);
      setFrameData(null);
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    }
  }, []);

  // ---- 用户交互（坐标换算 → CDP Input） ----
  const frameToViewport = useCallback(
    (clientX, clientY) => {
      const img = imgRef.current;
      if (!img || !img.naturalWidth) return null;
      const rect = img.getBoundingClientRect();
      // `<img>` 是 object-contain 等比缩放：getBoundingClientRect 返回整盒，
      // 宽高比不一致时上下/左右有 letterbox 黑边。必须按实际绘制区换算，
      // 绘制区之外的点击/移动不映射进页面坐标。
      const aspect = img.naturalWidth / img.naturalHeight;
      let drawnW = rect.width;
      let drawnH = drawnW / aspect;
      if (drawnH > rect.height) {
        drawnH = rect.height;
        drawnW = drawnH * aspect;
      }
      const offsetX = rect.left + (rect.width - drawnW) / 2;
      const offsetY = rect.top + (rect.height - drawnH) / 2;
      const px = clientX - offsetX;
      const py = clientY - offsetY;
      if (px < 0 || py < 0 || px > drawnW || py > drawnH) return null; // 黑边内
      let x = (px / drawnW) * img.naturalWidth;
      let y = (py / drawnH) * img.naturalHeight;
      // 页面缩放（pageScaleFactor≠1）时坐标换算到 CSS 像素
      const p = frameMeta && frameMeta.pageScaleFactor;
      if (p && p > 0 && p !== 1) {
        x /= p;
        y /= p;
      }
      return { x, y };
    },
    [frameMeta]
  );

  const sendInput = useCallback(async (payload) => {
    try {
      await invokeTauri('browser_input', { payload });
    } catch (e) {
      setError(typeof e === 'string' ? e : String(e));
    }
  }, []);

  const onFrameClick = useCallback(
    (e) => {
      // 点击画面即聚焦键盘容器（容器挂 onKeyDown，img 本身不可聚焦）。
      const host = imgRef.current && imgRef.current.parentElement;
      if (host && host.focus) host.focus();
      const p = frameToViewport(e.clientX, e.clientY);
      if (!p) return;
      sendInput({ type: 'click', x: p.x, y: p.y, button: 'left', clickCount: 1 });
    },
    [frameToViewport, sendInput]
  );

  const onFrameWheel = useCallback(
    (e) => {
      const p = frameToViewport(e.clientX, e.clientY);
      if (!p) return;
      sendInput({ type: 'wheel', x: p.x, y: p.y, deltaX: e.deltaX, deltaY: e.deltaY });
    },
    [frameToViewport, sendInput]
  );

  const onFrameMove = useCallback(
    (e) => {
      const now = Date.now();
      if (now - lastMove.current < 100) return; // hover 节流 100ms
      lastMove.current = now;
      const p = frameToViewport(e.clientX, e.clientY);
      if (!p) return;
      sendInput({ type: 'move', x: p.x, y: p.y });
    },
    [frameToViewport, sendInput]
  );

  const onFrameKeyDown = useCallback(
    (e) => {
      const keyMap = {
        Enter: { key: 'Enter', code: 'Enter', keyCode: 13 },
        Backspace: { key: 'Backspace', code: 'Backspace', keyCode: 8 },
        Escape: { key: 'Escape', code: 'Escape', keyCode: 27 },
        Tab: { key: 'Tab', code: 'Tab', keyCode: 9 },
        ArrowUp: { key: 'ArrowUp', code: 'ArrowUp', keyCode: 38 },
        ArrowDown: { key: 'ArrowDown', code: 'ArrowDown', keyCode: 40 },
        ArrowLeft: { key: 'ArrowLeft', code: 'ArrowLeft', keyCode: 37 },
        ArrowRight: { key: 'ArrowRight', code: 'ArrowRight', keyCode: 39 },
        ' ': { key: ' ', code: 'Space', keyCode: 32 },
      };
      const m = keyMap[e.key];
      if (!m) {
        // 可打印字符走 insertText（IME 级，任意 unicode 安全）
        if (e.key && e.key.length === 1) {
          sendInput({ type: 'insertText', text: e.key });
        }
        return;
      }
      e.preventDefault();
      sendInput({ type: 'key', key: m.key, code: m.code, keyCode: m.keyCode, text: e.key.length === 1 ? e.key : '' });
    },
    [sendInput]
  );

  // ---- 渲染 ----
  const shell = 'flex h-full flex-col overflow-hidden';
  const toolbarCls = `flex shrink-0 items-center gap-1 border-b px-2 py-1.5 ${
    isDark ? 'border-[#2A2B2E] bg-[#17181A]' : 'border-[#E5E7EB] bg-[#F8F9FA]'
  }`;
  const btnCls = `rounded-md p-1.5 transition-colors ${
    isDark ? 'text-[#B8B8B8] hover:bg-[#2A2B2E] hover:text-[#F2F2F2]' : 'text-[#555] hover:bg-[#ECECEC] hover:text-[#111]'
  }`;
  const iconBtn = (title, icon, onClick, disabled) => (
    <button title={title} className={btnCls} onClick={onClick} disabled={disabled} style={disabled ? { opacity: 0.35 } : undefined}>
      {icon}
    </button>
  );

  return (
    <div className={shell} data-testid="browser-view">
      {/* 工具条 */}
      <div className={toolbarCls}>
        {iconBtn(t.browserBack, <ChevronLeft size={17} />, () => runNav('browser_back'))}
        {iconBtn(t.browserForward, <ChevronLeft size={17} style={{ transform: 'rotate(180deg)' }} />, () => runNav('browser_forward'))}
        {iconBtn(t.browserRefresh, <RefreshCw size={16} />, () => runNav('browser_reload'))}
        {iconBtn(t.browserHome, <AppWindow size={16} />, () => navigate(HOME_URL))}
        <form
          className="mx-1 flex min-w-0 flex-1 items-center gap-1.5 rounded-md px-2 py-1"
          style={{
            background: isDark ? '#232428' : '#FFFFFF',
            border: `1px solid ${isDark ? '#3A3B3F' : '#D8DADC'}`,
          }}
          onSubmit={(e) => {
            e.preventDefault();
            navigate(urlInput);
          }}
        >
          <Globe size={14} style={{ opacity: 0.5 }} />
          <input
            className="w-full bg-transparent text-[13px] outline-none"
            style={{ color: isDark ? '#E8E8E8' : '#222' }}
            placeholder={t.browserUrlPlaceholder}
            value={urlInput}
            onChange={(e) => setUrlInput(e.target.value)}
            spellCheck={false}
            data-testid="browser-url-input"
          />
        </form>
        {iconBtn(t.browserNewTab, <Plus size={16} />, createTab)}
        {iconBtn(t.browserOpenExternal, <ExternalLink size={15} />, openExternal, !url || url === 'about:blank')}
        {iconBtn(t.browserStop, <XIcon size={16} />, stopBrowser)}
      </div>

      {/* 标签条 */}
      {tabs.length > 0 && (
        <div
          className={`flex shrink-0 items-center gap-1 overflow-x-auto px-2 py-1 ${
            isDark ? 'border-b border-[#2A2B2E] bg-[#1A1B1D]' : 'border-b border-[#E5E7EB] bg-white'
          }`}
        >
          {tabs.map((tab) => {
            const active = tab.session_id === (activeSession || activeSessionRef.current);
            return (
              <div
                key={tab.target_id}
                role="button"
                tabIndex={0}
                title={tab.url || tab.title}
                onClick={() => activateTab(tab.session_id)}
                className={`group flex max-w-[180px] cursor-pointer items-center gap-1 rounded-md px-2 py-1 text-[12px] ${
                  active
                    ? isDark
                      ? 'bg-[#2E2F33] text-[#F2F2F2]'
                      : 'bg-[#E9EBEE] text-[#111]'
                    : isDark
                      ? 'text-[#9A9A9A] hover:bg-[#232428]'
                      : 'text-[#666] hover:bg-[#F0F0F0]'
                }`}
              >
                <span className="truncate">{tab.title || tab.url || t.browserEmptyTab}</span>
                <button
                  className={`shrink-0 rounded p-0.5 opacity-0 group-hover:opacity-100 ${
                    isDark ? 'hover:bg-[#3A3B3F]' : 'hover:bg-[#DCDCDC]'
                  }`}
                  title={t.browserTabClose}
                  onClick={(e) => {
                    e.stopPropagation();
                    closeTab(tab.target_id);
                  }}
                >
                  <XIcon size={11} />
                </button>
              </div>
            );
          })}
        </div>
      )}

      {/* 画面：容器可聚焦并挂键盘转发（img 本身不可聚焦）。点击画面时聚焦容器，
          之后按键经 onFrameKeyDown → CDP Input 域转发到页面。 */}
      <div
        className="relative min-h-0 flex-1 overflow-hidden"
        style={{ background: isDark ? '#101113' : '#F4F5F6' }}
        tabIndex={0}
        onKeyDown={onFrameKeyDown}
        onFocus={(e) => {
          // 忽略 chrome-devtools 工具栏等自身聚焦，只处理画面容器焦点。
          if (e.target !== e.currentTarget) return;
          e.preventDefault();
        }}
      >
        {!running && (
          <div className="flex h-full items-center justify-center p-6 text-center text-[13px]" style={{ color: isDark ? '#9A9A9A' : '#777' }}>
            {error ? (
              <div>
                <div>{t.browserNoChrome}</div>
                <div className="mt-2" style={{ opacity: 0.6 }}>{error}</div>
              </div>
            ) : (
              <div>
                <div className="mb-2"><Maximize2 size={28} style={{ opacity: 0.4, margin: '0 auto' }} /></div>
                <div>{t.browserLoading}</div>
                <div className="mt-2" style={{ opacity: 0.6, maxWidth: 360 }}>{t.browserNotRunning}</div>
              </div>
            )}
          </div>
        )}
        {running && frameData && (
          <img
            ref={imgRef}
            src={`data:image/jpeg;base64,${frameData}`}
            alt=""
            className="block h-full w-full select-none object-contain"
            style={{ cursor: 'default', touchAction: 'none' }}
            onClick={onFrameClick}
            onWheel={onFrameWheel}
            onMouseMove={onFrameMove}
            draggable={false}
          />
        )}
        {running && !frameData && (
          <div className="flex h-full items-center justify-center text-[13px]" style={{ color: isDark ? '#9A9A9A' : '#777' }}>
            {t.browserLoading}
          </div>
        )}
      </div>
    </div>
  );
}
