# L1 scenario: `long_output_1500`

## meta

- mode: `Yolo` / phase: `None`
- elapsed: **85.4s**
- timed_out: false
- tool_call_histogram: `{}`
- text_chars: 4822

## user prompt

```text
写一份 1500+ 字的中文技术文档,主题:Rust 异步运行时 Tokio 的工作机制。必须覆盖:(1) Reactor / Executor 双层调度;(2) Waker 与 Future poll 机制;(3) work-stealing scheduler 原理;(4) 一段简短代码示例。直接在对话里给完整文档内容,不需要 write_file。
```

## tool / event timeline

- `[+85.4s]` **turn_complete** status=Completed usage=in:13499/out:2168

## assistant final text

```
好的，我来写一份关于 Tokio 异步运行机制的技术文档。

## Rust 异步运行时 Tokio 工作机制深度解析

### 一、引言

Tokio 是 Rust 生态中最成熟的异步运行时（Async Runtime），其设计直接决定了 Rust 异步代码的性能与行为。理解 Tokio 的内部机制，是写出高效异步程序的前提。本文从调度架构、Waker 机制、Future 生命周期到 Work-Stealing 负载均衡，层层剖析 Tokio 的工作原理。

---

### 二、Reactor / Executor 双层调度架构

Tokio 的核心架构可以抽象为两个独立但协同工作的层次：**Executor（执行器）** 和 **Reactor（反应器）**。这个分层设计是理解 Tokio 一切行为的基础。

**Executor** 负责线程本地的任务调度。每个工作线程上都有一个独立的 executor，其核心是一个线程安全的任务队列（多生产者单消费者，MPSC）。executor 从队列中取出任务并执行，这些任务本质上是实现了 `Future` trait 的"句柄"——它们本身并不包含实际工作逻辑，而是指向 Reactor 上某个被监控事件的回调。当 executor 执行一个 I/O 任务时，它实际做的事情是：将事件注册到 Reactor，然后从队列中取出下一个任务继续执行。

**Reactor** 负责底层 I/O 事件的多路复用。在 Linux 上，它基于 `epoll`（macOS 上是 `kqueue`）。Reactor 通过 Rust 标准库中的 `mio` crate 与操作系统交互，注册文件描述符的可读/可写事件。当操作系统通知某个文件描述符就绪时，Reactor 会找到对应的任务注册记录，并将该任务作为一个新的 `Join` 任务 push 到相应 executor 的队列中。

两个层次通过 `Waker` 机制衔接：Reactor 检测到事件就绪后，调用 `Waker::wake()` 将关联的 future 标记为可就绪状态；executor 的 poll 循环在 future 返回 `Poll::Pending` 时，将 future 对应的 Waker 注册到 Reactor，然后去执行队列中的下一个任务。这种"事件驱动 + 线程本地调度"的架构，使得 Tokio 能够以极低的开销管理数以百万计的并发任务。

---

### 三、Waker 与 Future Poll 机制

#### 3.1 Future 的生命周期

Rust 异步的核心抽象是 `Future` trait：

```rust
pub trait Future {
    type Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output>;
}
```

`Future` 本身不是线程、不是协程，而是一种**状态机**。一个 `Future` 在被 poll 时会经历以下状态流转：

- **未 poll 过** → 第一次 `poll()` 返回 `Poll::Pending`，表示"我现在做不完，等事件到来再说"
- **已注册 Waker** → `poll()` 内部将 `Context` 中携带的 `Waker` 注册到某个外部事件源（如文件描述符的 epoll 集合），然后返回 `Pending`
- **事件就绪** → 事件源触发，`Waker::wake()` 被调用，executor 的调度器将对应的 future 重新放入任务队列
- **再次 poll** → 此时 future 从上次中断的地方继续执行，若完成则返回 `Poll::Ready(output)`

关键点在于：**一个 future 可以被多次 poll**（例如 tokio::sync::oneshot 的 receiver 会被多个 waiter 共享），因此 `poll()` 必须是幂等且线程安全的。

#### 3.2 Waker 的底层实现

`Waker` 是 Rust 异步运行时中"回调"概念的泛型抽象。它实现了 `Clone` 和 `Send`，可以在线程间安全传递。其本质是一个 `(vtable, data)` 的双指针结构：

- **data 指针**：指向具体的唤醒目标（在 Tokio 中通常是一个 `RawWakerVTable` 关联的句柄）
- **vtable 指针**：指向虚函数表，定义了 `clone`、`wake`、`wake_by_ref`、`drop` 操作

当 `Waker::wake()` 被调用时，虚函数表的 `wake` 函数被触发。在 Tokio 中，这个函数的典型行为是：**将关联的任务 ID push 到工作线程的 executor 队列中**。如果任务已在队列中，重复 push 是安全的（队列本身会去重或接受重复）。

#### 3.3 poll 与 Waker 的协作流程

整个循环的完整流程如下：

```
1. executor 从队列取出一个 Task
2. executor 调用 task.poll(&mut Context)
3. Future::poll 内部:
   a. 尝试完成工作（非阻塞路径）→ 返回 Ready，任务结束
   b. 需要等待 I/O → 注册 Waker 到 Reactor，返回 Pending
4. executor 收到 Pending → 从队列取下一个 Task 执行
5. Reactor 检测到 I/O 事件 → 调用 Waker::wake()
6. Waker wake 操作 → Task 被 push 回 executor 队列
7. 回到步骤 1
```

这个循环保证了：**永远有工作可做，永远不会空转**。executor 队列非空时，线程就在消费任务；队列空时，线程阻塞在 Reactor 上等待新事件。

---

### 四、Work-Stealing Scheduler 原理

Tokio 的调度器采用 **Work-Stealing（工作窃取）** 策略，这是其处理并发负载的核心机制。

#### 4.1 线程本地队列

每个工作线程拥有一个 **线程本地 FIFO 队列**（deque），任务从 executor push 时，直接进入当前线程的本地队列。这种设计确保了极高的缓存局部性——线程 poll 的 future 极大概率驻留在自己的 L1/L2 缓存中。

#### 4.2 Steal 与 Push 操作

工作线程从自己的队列中操作时，采用以下两种策略：

- **本地 poll**：从本地队列头部取出一个任务执行。如果队列非空，继续从头部取（`pop`）
- **Steal（窃取）**：当本地队列为空时，线程会遍历其他线程的队列，尝试从**尾部**窃取任务。窃取通常是批量操作（一次窃取约一半的任务），以减少锁竞争

这种"头部取、尾部偷"的设计避免了两端同时操作同一 deque 产生的竞争——本地 poll 和 steal 操作在 deque 的两端进行，互不干扰。

#### 4.3 全局队列与任务注入

除了线程本地队列，Tokio 还有一个**全局 MPSC 队列**。`tokio::spawn()` 时如果调用线程不是工作线程（例如 `main` 线程），或者 executor 的 `spawn` 方法被从外部调用，任务会进入全局队列，再由某个工作线程从全局队列中 pop 出来，放入本地队列执行。

#### 4.4 工作窃取的好处

- **负载均衡**：繁忙的线程任务堆积，空闲线程主动"偷"任务，自动平衡负载
- **减少全局锁**：大部分任务在线程本地队列上操作，无需任何锁
- **缓存友好**：线程优先执行自己的任务，复用已加载到缓存的 future 状态
- **弹性扩展**：新增工作线程后，它会主动从其他线程窃取任务，快速达到满载

---

### 五、代码示例

以下代码演示了上述机制的实际表现：

```rust
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    println!("Server listening on 127.0.0.1:8080");

    loop {
        // 每个 accept 产生一个 Task，push 到 executor 队列
        let (mut socket, addr) = listener.accept().await?;
        
        // .await 返回 Pending 时：
        // 1. socket 的 Readable 事件注册到 Reactor (epoll)
        // 2. 当前 Future 的 Waker 被存入 Reactor 关联表
        // 3. executor 去队列取下一个任务继续执行
        
        tokio::spawn(async move {
            let mut buf = vec![0u8; 1024];
            let n = socket.read(&mut buf).await?;
            socket.write_all(&buf[..n]).await?;
            Ok::<_, std::io::Error>(())
        });
    }
}
```

在这个示例中：
- `TcpListener::bind` 将 socket fd 注册到 Reactor
- `accept().await` 让 executor 去执行队列中的下一个任务
- 新连接到达时，Reactor 通过 Waker 将 spawn 出去的 task 重新入队
- 多个连接可以并发处理，底层共享 epoll 集合和线程池

---

### 六、总结

Tokio 的架构可以概括为：**Reactor 负责感知事件，Executor 负责调度执行，Waker 负责两者之间的衔接，Work-Stealing 负责跨线程负载均衡**。这四个组件协同工作，使得 Rust 异步运行时能够在不依赖 GC、不依赖线程栈的前提下，实现高性能的并发编程模型。

理解这套机制后，你就能解释很多"为什么"：为什么 `.await` 之后代码不会阻塞线程、为什么 async 函数必须是 `Send` 才能跨线程、为什么 Tokio 的默认线程数是 CPU 核心数、以及为什么在单线程 executor 上使用阻塞调用会导致整个运行时饿死。这些问题的根源，都可以追溯到 Reactor/Executor 双层架构和 Work-Stealing 调度策略的设计之中。
```
