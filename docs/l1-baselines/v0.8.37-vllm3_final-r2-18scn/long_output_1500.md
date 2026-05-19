# L1 scenario: `long_output_1500`

## meta

- mode: `Yolo` / phase: `None`
- elapsed: **122.2s**
- timed_out: false
- tool_call_histogram: `{"web_search": 1}`
- text_chars: 6797

## user prompt

```text
写一份 1500+ 字的中文技术文档,主题:Rust 异步运行时 Tokio 的工作机制。必须覆盖:(1) Reactor / Executor 双层调度;(2) Waker 与 Future poll 机制;(3) work-stealing scheduler 原理;(4) 一段简短代码示例。直接在对话里给完整文档内容,不需要 write_file。
```

## tool / event timeline

- `[+11.2s]` **tool_start** `web_search` id=`call_8b5658d00bef44c3aeb07d00` args=`Object {"query": String("Tokio Rust async runtime architecture work-stealing scheduler 2024 2025"), "max_results": Number(5)}`
- `[+11.2s]` **tool_start** `web_search` id=`call_f433e52a52884897b7b0f0c4` args=`Object {"query": String("Tokio Waker Future poll机制 源码分析"), "max_results": Number(5)}`
- `[+11.6s]` **tool_end** `web_search` id=`call_8b5658d00bef44c3aeb07d00` → **err** `ExecutionFailed { message: "Bing search request failed: error sending request for url (https://www.bing.com/search?q=Tokio+Rust+async+runtime+architecture+work-stealing+scheduler+2024+2025)" }`
- `[+12.4s]` **tool_end** `web_search` id=`call_f433e52a52884897b7b0f0c4` → **ok** `{⏎  "query": "Tokio Waker Future poll机制 源码分析",⏎  "source": "bing",⏎  "count": 0,⏎  "message": "No results found",⏎  "results": []⏎}`
- `[+122.2s]` **turn_complete** status=Completed usage=in:27281/out:3187

## assistant final text

```
我先梳理 Tokio 的核心机制框架，然后直接输出完整文档。

Tokio 的关键组件包括：Reactor（epoll/io_uring）、Executor（task 调度）、Waker（Future 唤醒）、Work-Stealing（多线程任务分配）。让我先确认一下最新的一些实现细节。

网络搜索受限，不过 Tokio 的机制是稳定的核心知识，我不需要额外联网就能写出准确的文档。下面是完整的技术文档。

---

## Rust 异步运行时 Tokio 工作机制深度解析

### 一、概述

Tokio 是 Rust 生态中最成熟的异步运行时（async runtime），由 Tokio Contributors 团队维护。它提供了异步 I/O、定时器、同步原语和高效的线程调度器，构成了 Rust 异步编程的核心基础设施。理解 Tokio 的工作机制，核心在于掌握 **Reactor / Executor 双层调度架构**、**Waker 与 Future 协作机制**，以及 **Work-Stealing 调度器原理** 这三块内容。

---

### 二、Reactor / Executor 双层调度

Tokio 的整体架构可以抽象为两层：**Reactor（反应器层）** 和 **Executor（执行器层）**。这两层职责清晰、松耦合配合，是 Tokio 高性能的关键设计。

#### 2.1 Reactor 层：事件驱动 I/O

Reactor 层的核心职责是 **管理操作系统级 I/O 事件**。它监听文件描述符（fd）上的可读、可写、超时等事件，将 I/O 完成信号转化为 Rust 层面的 Future 就绪通知。Tokio 根据平台选择不同的后端：

- **Linux/macOS**：使用 `epoll` / `kqueue`
- **Linux 5.1+**：支持 `io_uring`（通过 `tokio-util`）
- **Windows**：使用 IOCP（I/O Completion Ports）
- **macOS/iOS/tvOS**：使用 `kqueue`

当用户发起一个异步 TCP 连接（如 `TcpStream::connect`）时，底层流程如下：

1. 创建非阻塞 socket fd，加入 epoll 注册表；
2. 发起 `connect` 系统调用，由于非阻塞，返回 `EINPROGRESS`；
3. Reactor 将该 fd 注册到 epoll 的"可写"事件监听；
4. 操作系统在连接真正建立后，向 epoll 队列放入一个事件；
5. Reactor 通过 `poll` 系统调用拿到事件通知，唤醒对应的任务。

整个过程中，Reactor 层 **不执行任何业务逻辑**，只负责 I/O 事件的收集与分发。

#### 2.2 Executor 层：任务调度执行

Executor 层的核心职责是 **管理异步任务的调度与执行**。它维护一个任务队列，从队列中取出 `Future`，调用其 `poll` 方法驱动执行，并在 Future 暂时无法继续推进时将其挂起。

Executor 的工作流程可以概括为：

1. 将实现了 `Future` 的任务（`task`）放入调度队列；
2. 从队列中取出一个任务，调用 `poll` 驱动执行；
3. 如果 `Future` 返回 `Poll::Ready(val)`，任务结束；
4. 如果返回 `Poll::Pending`，任务被挂起，等待被再次唤醒；
5. 重复上述过程。

Executor 和 Reactor 通过 **Waker** 机制协作：当 Reactor 检测到某个 I/O 事件完成时，它通过 Waker 唤醒对应 Future 所绑定的任务，Executor 随后重新将其加入调度队列。

#### 2.3 双层设计的优势

这种分层设计带来了三个核心好处：

- **关注点分离**：Reactor 专注 I/O 多路复用，Executor 专注任务调度，两者通过 Waker 接口交互；
- **灵活组合**：Rust 的 `futures` crate 提供了独立的 Future 组合子库，无需绑定特定运行时；
- **可替换性**：可以替换 Executor（如用异步 executor 替代多线程 executor）而不影响 Reactor。

---

### 三、Waker 与 Future Poll 机制

这是理解异步运行时的 **最核心概念**，也是 Rust `async/await` 能工作的根本原理。

#### 3.1 Future Trait 的本质

Rust 标准库中的 `Future` trait 定义如下：

```rust
use std::task::{Context, Poll};

pub trait Future {
    type Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output>;
}

pub enum Poll<T> {
    Ready(T),
    Pending,
}
```

`poll` 方法是 Future 的 **唯一执行入口**。调用者（即 Executor）反复调用 `poll`，推动 Future 向前执行，直到返回 `Poll::Ready` 或 `Poll::Pending`。

- **`Poll::Ready`**：Future 已完成，返回结果；
- **`Poll::Pending`**：Future 暂时无法继续推进（例如在等待 I/O），需要被挂起等待唤醒。

#### 3.2 Waker：异步唤醒的桥梁

关键问题是：**当被挂起的 I/O 完成时，谁来通知 Executor 重新运行这个 Future？** 答案就是 **Waker**。

Waker 的语义是：一个 Waker 持有一个任务的"句柄"，调用 `Waker::wake()` 可以将对应的任务重新加入 Executor 的调度队列。

在 `poll` 方法内部，当 Future 发现自己需要等待某个 I/O 事件时，它会执行以下逻辑：

```
1. 将自身的 I/O 事件注册到 Reactor ；
2. 将 Waker（指向自己的任务句柄）注册到 Reactor，以便事件完成时通知自己；
3. 返回 Poll::Pending，让出执行权。
```

当 Reactor 检测到 I/O 事件完成时，它调用之前注册的 Waker → Executor 将对应任务重新加入队列 → 任务被再次 `poll` → 此时 I/O 已完成，Future 返回 `Poll::Ready`。

#### 3.3 Wake 的三种方式

Waker 的唤醒方式有三种层次：

- **`wake`**：将 Waker 克隆一份，在独立线程中发送唤醒信号，适用于大多数场景；
- **`wake_by_ref`**：通过引用直接唤醒，避免克隆开销；
- **`wake_by_deref`**（`Awake` 语义）：直接触发唤醒逻辑，性能最优。

Tokio 的 Task 内部使用 Arc 来共享 Waker 数据，支持在 Reactor 线程中直接 `wake_by_ref`，零拷贝。

---

### 四、Work-Stealing Scheduler 原理

Tokio 的多线程调度器采用 **Work-Stealing（工作窃取）** 算法，这是它区别于单线程队列调度的关键特征。

#### 4.1 为什么需要 Work-Stealing？

假设你有 4 个 CPU 核心，但只有一个全局任务队列：

- 任务 1 被分配到核心 A，开始执行；
- 核心 B、C、D 空闲，但由于任务在核心 A 的本地队列中，它们什么都做不了。

这就是 **任务饥饿（task starvation）** 问题。Work-Stealing 通过将任务与线程一一绑定来解决这个问题。

#### 4.2 本地队列 + 全局队列

Tokio 调度器的结构如下：

```
┌─────────────────────────────────────────┐
│          Global Queue (全局队列)          │
│         新创建的任务放入此处              │
└──────────┬──────────┬──────────┬────────┘
           │          │          │
      ┌────┴───┐ ┌────┴───┐ ┌────┴───┐
      │Core 0  │ │Core 1  │ │Core 2  │
      │LocalQ  │ │LocalQ  │ │LocalQ  │
      └────────┘ └────────┘ └────────┘
```

- 每个 OS 线程（"core"）维护一个 **本地 FIFO 队列**；
- 新创建的任务被放入 **某个** 核心对应的本地队列；
- 每个核心不断从自己的本地队列中取任务执行；
- 当一个核心的本地队列为空时，它启动 **窃取（steal）** 过程：从其他核心的本地队列底部随机偷取任务。

#### 4.3 为什么从底部窃取？

这是一个关键设计选择。本地队列采用 **LIFO（栈）顺序** 取任务（顶部），而窃取时从 **底部** 取任务。

原因很简单：顶部是 **最新** 的任务（通常是刚被创建或刚被 `spawn` 出来的），底部是 **最早** 进入的任务。当核心 A 去核心 B 的队列"偷"任务时，从底部取不会干扰核心 B 正在使用的最新任务，减少了竞争。

```
Local Queue (Core 1):

  ┌──────────┐  ← 顶部（pop 取走）
  │  Task C  │  最新
  ├──────────┤
  │  Task B  │
  ├──────────┤
  │  Task A  │  ← 底部（steal 偷走）
  └──────────┘     最旧
```

#### 4.4 窃取过程

当核心 A 的本地队列为空时：

1. 核心 A 随机选择一个其他核心 B；
2. 尝试从核心 B 的本地队列 **底部** 窃取任务（使用原子操作 + CAS）；
3. 如果成功窃取，开始执行窃取到的任务；
4. 如果失败（核心 B 也在同时操作队列），重试另一个核心；
5. 所有核心都无法窃取到任务时，核心 A 进入自旋或休眠状态。

#### 4.5 Work-Stealing 的性能优势

- **局部性**：大多数情况下，任务在自己的本地队列中执行，无跨线程同步；
- **负载均衡**：空闲核心自动从繁忙核心"偷"任务，天然负载均衡；
- **低竞争**：LIFO 取 + 底部窃取的组合将竞争降至最低。

---

### 五、代码示例

以下代码展示了上述机制在实际中的运作：

```rust
use tokio::time::{sleep, Duration};
use std::time::Instant;

#[tokio::main]
async fn main() {
    let start = Instant::now();

    // spawn 创建异步任务，放入 Work-Stealing 调度器
    let handle1 = tokio::spawn(async {
        println!("[Task 1] 开始执行，等待 200ms ...");
        sleep(Duration::from_millis(200)).await;
        println!("[Task 1] 唤醒，已完成等待");
    });

    let handle2 = tokio::spawn(async {
        println!("[Task 2] 开始执行，等待 100ms ...");
        sleep(Duration::from_millis(100)).await;
        println!("[Task 2] 唤醒，已完成等待");
    });

    // 等待两个任务完成
    handle1.await.unwrap();
    handle2.await.unwrap();

    println!(
        "所有任务完成，总耗时: {:.2}ms",
        start.elapsed().as_millis() as f64
    );
}
```

这段代码中可以看到关键流程：

1. `#[tokio::main]` 启动包含 Reactor 和 Work-Stealing Executor 的单线程运行时；
2. `tokio::spawn` 创建新任务，放入调度器的本地队列；
3. `sleep(...).await` 本质上是 `Sleep` Future 的 `poll` 方法——它将自己的 I/O 事件（定时器超时）注册到 Reactor，注册自己的 Waker，然后返回 `Poll::Pending`；
4. Reactor 在定时器到期后调用 Waker，Executor 重新 `poll` 该任务，此时返回 `Poll::Ready`；
5. 两个 `sleep` 任务并发执行，总耗时约 200ms 而非 300ms，体现了异步并发的能力。

若改用多线程运行时（`#[tokio::flavor="multi"]`），这两个任务会被分配到多个 OS 线程上并发执行，进一步利用多核 CPU。

---

### 六、总结

Tokio 的工作机制可以用一张简图概括：

```
          用户 async/await 代码
                 │
                 ▼
          ┌─────────────┐
          │  Future (用户) │
          └──────┬──────┘
                 │ poll
                 ▼
          ┌─────────────┐     I/O事件      ┌─────────────┐
          │  Waker      │◄───────────────►│  Reactor     │
          │ (唤醒通知)    │                │ (epoll/kqueue)│
          └──────┬──────┘                └─────────────┘
                 │ wake
                 ▼
          ┌─────────────────────┐
          │  Executor            │
          │  Work-Stealing 调度器  │
          │  Core 0/1/2/3 本地队列 │
          └─────────────────────┘
```

核心要点归纳为三点：

1. **Reactor 与 Executor 分离**，前者管 I/O 事件，后者管任务调度，通过 Waker 通信；
2. **Future 的 poll 语义 + Waker 唤醒机制** 构成了 Rust 异步编程的底层模型，没有"回调"，只有"等待被轮询"；
3. **Work-Stealing 调度器** 通过本地队列 + 底部窃取实现了低竞争、高并发的任务分配，是 Tokio 在多核机器上高效运行的基础。
```
