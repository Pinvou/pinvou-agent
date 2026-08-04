/**
 * 串行刷新一份运行中的 transcript：任一时刻最多一个读取请求；agent 结束后
 * `active` 可为布尔值或按本次结果判断的函数；终态只做最后一次读取，不再
 * 安排 timer。
 *
 * 落盘与实时状态的合并不在前端做：底座 worker ledger 是终态权威，
 * `transcripts::list`（Rust）已按它投影，面板轮询拿到的就是合并结果。
 */
export function startTranscriptPolling({
  read,
  onMessages,
  active,
  intervalMs = 1500,
  schedule = (callback, delay) => globalThis.setTimeout(callback, delay),
  cancel = (timer) => globalThis.clearTimeout(timer),
}) {
  let stopped = false;
  let timer = null;

  async function refresh() {
    // null = 本次读取失败（桥已区分"空表"与"失败"）。原样透传：调用方要
    // 保留上次有效数据并显示"读取失败重试中"，不能伪装成没有记录。
    let result = null;
    try {
      const next = await read();
      result = Array.isArray(next) ? next : null;
    } catch {
      result = null;
    }
    if (stopped) return;
    onMessages(result);
    const keepPolling = typeof active === 'function' ? active(result) : active;
    if (keepPolling) timer = schedule(refresh, intervalMs);
  }

  void refresh();
  return () => {
    stopped = true;
    if (timer !== null) cancel(timer);
  };
}
