# L1 scenario: `long_output_1500`

## meta

- mode: `Yolo` / phase: `None`
- elapsed: **106.7s**
- timed_out: false
- tool_call_histogram: `{}`
- text_chars: 6804

## user prompt

```text
写一份 1500+ 字的中文技术文档,主题:Rust 异步运行时 Tokio 的工作机制。必须覆盖:(1) Reactor / Executor 双层调度;(2) Waker 与 Future poll 机制;(3) work-stealing scheduler 原理;(4) 一段简短代码示例。直接在对话里给完整文档内容,不需要 write_file。
```

## tool / event timeline

- `[+106.7s]` **turn_complete** status=Completed usage=in:13499/out:2883

## assistant final text

```
## Rust 异步运行时 Tokio 工作机制详解

### 一、引言:异步编程的核心矛盾

Rust 的异步编程基于 `Future` trait,其核心设计哲学是"零成本抽象"——异步代码不应引入运行时的额外开销。然而,`Future` 本身只是一个惰性状态机,它不会自己执行任何代码。需要一个运行时来"驱动"这些状态机,协调它们的执行,这就是 **Tokio** 的角色。

Tokio 不是 Rust 标准库的一部分,但它已经成为 Rust 异步生态的事实标准。理解 Tokio 的工作机制,是理解整个 Rust 异步模型的关键。

本文从底层到上层,逐层拆解 Tokio 的四项核心机制:Reactor 与 Executor 的双层调度架构、Waker 与 Future 的 poll 驱动模型、work-stealing 调度器原理,以及一段可追踪执行流程的代码示例。

---

### 二、Reactor / Executor 双层调度

Tokio 的架构遵循 **Reactor 模式**(Reactor pattern)与 **Executor 模式**(Executor pattern)的分离,这是理解 Tokio 的第一道门槛。

**Reactor(反应器)** 的职责是 I/O 事件的多路复用。它底層调用 `epoll`(Linux)、`kqueue`(macOS/BSD)或 `IOCP`(Windows)等操作系统提供的异步 I/O 接口,将网络读写、文件 I/O 等事件注册到内核事件表中。当事件就绪时,Reactor 将其产出为一个信号,通知相关的 Future 可以恢复执行。Reactor 本身**不执行任何用户代码**,它只负责"发现什么时候可以执行"。

**Executor(执行器)** 的职责是驱动 `Future` 的执行。它从工作队列中取出 Future,调用其 `poll` 方法推进状态机。如果 Future 返回 `Poll::Pending`,说明它还在等待某种资源(网络数据、锁、计时器等),Executor 会将其挂起并处理下一个 Future。如果 Future 返回 `Poll::Ready(value)`,说明它已完成,Executor 会触发与该 Future 关联的 Waker,通知等待方结果已就绪。

这两层的分离带来一个关键的设计优势:**I/O 事件发现与计算调度解耦**。Reactor 专注于高效地管理成千上万的并发 I/O 连接,Executor 专注于高效地调度计算任务。两者通过 Waker 机制衔接——当 I/O 事件就绪时,Reactor 通过 Waker 唤醒对应的 Future,让 Executor 去驱动它执行。

---

### 三、Waker 与 Future Poll 机制

这是 Tokio(以及整个 Rust 异步生态)最精妙、也最容易被误解的部分。

#### 3.1 Future 是一个状态机

`Future` trait 的定义极其简洁:

```rust
trait Future {
    type Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output>;
}
```

`poll` 方法被调用时,Future 检查自己的状态:
- 如果依赖的资源**已就绪**(比如 Socket 上有数据可读),返回 `Poll::Ready(output)`,表示计算完成。
- 如果资源**尚未就绪**,返回 `Poll::Pending`,并将一个 **Waker** 注册到等待队列中,然后返回。

关键在于:一旦返回 `Pending`,**没有任何人会自动再次调用 `poll`**。Waker 就是那个"自动唤醒者"。

#### 3.2 Waker 的作用

Waker 的核心方法只有一个:

```rust
trait Waker {
    fn wake(self);
    fn wake_by_ref(&self);
}
```

当 Future 返回 `Pending` 时,它会通过 `Context` 拿到一个 Waker 引用,并将这个 Waker 注册到对应的等待源中。比如一个 `TcpStream::read` 调用,会将 Waker 注册到 Reactor 中,关联到具体的文件描述符。当网络数据到达、操作系统通知 I/O 就绪时,Reactor 就会调用这个 Waker 的 `wake` 方法。

`wake` 做了什么?它将对应的 Future(准确地说是 Future 被提交到的那个 Task)重新放回 Executor 的工作队列中。下一次循环时,Executor 会再次调用这个 Future 的 `poll`——此时资源已就绪,返回 `Ready`。

#### 3.3 一个完整的 poll 生命周期

```
1. Executor 调用 Future.poll(cx)
2. Future 检查资源 → 未就绪
3. Future 通过 cx.waker() 注册 Waker 到等待源(如 Reactor)
4. Future 返回 Poll::Pending
5. Executor 切换到下一个 Task
... (一段时间过去) ...
6. I/O 事件就绪,Reactor 调用 Waker::wake()
7. Task 被重新入队
8. Executor 再次调用 poll → 资源就绪 → 返回 Poll::Ready
```

整个过程中,**没有任何线程阻塞**。Waker 是一个 `Send + Sync` 的对象,通常内部持有一个 `Arc`,指向封装了 Task ID 和队列句柄的数据结构。`wake` 本质上就是一个原子操作 + 一次无锁入队。

---

### 四、Work-Stealing Scheduler 原理

Tokio 的多线程调度器采用 **work-stealing(工作窃取)** 策略,这是它能够在多核 CPU 上实现高效并发的核心机制。

#### 4.1 每个线程绑定一个局部队列

Tokio 创建一个线程池,每个工作线程都绑定一个**本地双端队列(local deque)**。当你在某个线程中 `.await` 一个 Future 时,这个 Future 被包装成一个 Task,放入**当前线程**的局部队列中。

```
Thread 0: [Task A, Task B, Task C] ← local deque
Thread 1: [Task D, Task E]         ← local deque
Thread 2: []                       ← empty
Thread 3: [Task F]                 ← local deque
```

#### 4.2 Push 与 Pop:双端队列的两端

每个局部双端队列支持两个方向的操作:
- **Push/Pop(栈端)**:线程将自己生成的新 Task 压入栈顶,并从栈顶弹出执行。这最大化了**局部性**——最近生成的 Task 往往与当前 Task 有强关联,放在栈顶可以立刻执行。
- **Steal(队列端)**:当线程 A 的空了自己的局部队列时,它会从**随机其他线程**的局部队列**底部**窃取一半的 Task。

```
Thread 0 空了,从 Thread 1 底部窃取 → 偷走 Task E
Thread 0: [Task A, Task B]    Thread 1: [Task D]
```

#### 4.3 为什么"窃取一半"而不是全部?

这是 work-stealing 的关键优化。假设线程 A 从线程 B 偷走了所有 Task,那么线程 B 可能又立刻去偷 A 的 Task,造成**振荡**(thrashing)。窃取一半保证了双方都有活干,减少了无意义的竞争。

#### 4.4 两种工作队列:本地队列与全局队列

Tokio 实际上维护了两层队列:
- **本地队列**:每个线程独占,push/pop 无锁,性能极高。
- **全局队列**:当某个线程的本地队列满到一定程度,或者全局有新的 Task 需要分发时,会通过全局队列间接入。全局队列使用有锁结构,但访问频率远低于本地队列。

#### 4.5 与 Golang goroutine 调度的对比

理解 work-stealing 的另一种方式是对比 Go 的 M:N 调度。Goroutine 的调度器使用 **G-M-P 模型**,每个 OS 线程绑定一个 run queue,M 空闲时从全局队列或其他 M 的 run queue 中 steal。Tokio 的 work-stealing 与 Go 的思路高度相似,但 Tokio 的 Task 调度粒度更细——每个 `.await` 点都可能产生新的 Task,而 Go 的 goroutine 调度主要依赖于 channel 操作和 `runtime.Gosched`。

---

### 五、代码示例:追踪执行流程

下面这段代码展示了 Tokio 异步编程的基本模式,并在注释中标注了每一步背后 Runtime 发生了什么。

```rust
use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ===== ① Runtime 启动 =====
    // `#[tokio::main]` 宏展开后创建了单线程 Runtime。
    // Runtime 包含一个 Executor(单线程版本)和一个 Reactor。
    // Executor 持有本地双端队列,Reactor 注册 epoll 实例。

    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    // ===== ② 第一次 await =====
    // TcpListener::bind 内部创建一个 I/O 注册。
    // bind() 返回一个 Future,Executor 调用 .poll()。
    // 绑定 Socket 是同步可完成的,所以第一次 poll 就返回 Ready。
    // 如果网络地址还在等待,它会注册 Waker 到 Reactor 并返回 Pending。

    loop {
        let (mut socket, addr) = listener.accept().await?;
        // ===== ③ 第二次 await =====
        // accept() 返回一个 Accept Future。
        // 第一次 poll:检查监听 Socket 上是否有连接。
        //   - 无连接 → 注册 Waker 到 Reactor(epoll 监听 fd),返回 Pending。
        //   - Executor 继续处理本地队列中的其他 Task。
        //   - 有连接到达 → 操作系统通知 Reactor → Reactor 调用 Waker::wake()
        //   - Task 重新入队 → Executor 再次 poll → 返回 Ready。
        //
        // 注意:accept 的 Future 内部包含连接建立的完整状态机,
        // 从 epoll 检测到 Socket 握手的完成,全部由 Future 状态机管理。

        println!("新连接: {}", addr);

        // ===== ④ 并发处理:spawn =====
        // 这里 spawn 创建一个新 Task,放入 Executor 的本地队列。
        // 单线程 Runtime 下,它与当前循环代码共享同一个队列,
        // Executor 按队列顺序交替执行。
        // 如果是多线程 Runtime (`runtime_builder.build().unwrap()`)
        // 则 Task 会被放入全局队列,work-stealing 调度器决定由哪个
        // 工作线程执行它。
        tokio::spawn(async move {
            let mut buf = vec![0; 1024];
            // ===== ⑤ 第三次 await =====
            // read 的 Future 将 socket fd 注册到 Reactor。
            // poll 返回 Pending 后,Task 挂起。
            // 数据到达 → Waker 唤醒 → 再次 poll → 返回 Ready。
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                return; // 连接关闭
            }
            socket.write_all(&buf[..n]).await.unwrap();
            // write 类似:注册到 Reactor,等待写缓冲区有空闲。
        });
    }
}
```

#### 执行流程总结

| 步骤 | 代码位置 | Reactor 状态 | Executor 状态 |
|------|---------|-------------|--------------|
| ① | `#[tokio::main]` | 初始化 epoll,无注册 | 初始化本地队列 |
| ② | `bind().await` | 无事件等待 | bind 完成,返回 Ready |
| ③ | `accept().await` | 监听 fd 加入 epoll | 返回 Pending,Task 挂起 |
| ③ | 连接到达 | epoll 返回 fd 可读 | Waker wake,Task 入队,poll 返回 Ready |
| ④ | `tokio::spawn` | 无变化 | 新 Task 入队 |
| ⑤ | `read().await` | socket fd 加入 epoll | 返回 Pending,Task 挂起 |

---

### 六、总结

Tokio 的架构可以概括为三句话:

1. **Reactor 负责"什么时候能执行"**——通过操作系统事件接口高效管理并发 I/O,不执行用户代码。
2. **Executor 负责"执行什么、按什么顺序"**——通过 poll 驱动 Future 状态机,通过 Waker 实现非阻塞的事件通知。
3. **Work-stealing 负责"在多少线程上执行"**——每个线程绑定局部队列,空闲线程从繁忙线程窃取任务,最大化多核利用率。

这三层机制相互独立又紧密协作:Reactor 的事件驱动了 Waker 的触发,Waker 的触发驱动了 Executor 的任务调度,而 Executor 的调度策略由 work-stealing 决定。理解了这个分层,也就理解了 Rust 异步模型为什么能够实现 C++ 异步框架(如 Boost.Asio)一样的性能,同时保持内存安全和零成本抽象。
```
