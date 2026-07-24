//! 一次性 spike：验证本机 X11 下 device_query 能读全局鼠标坐标 + 左键状态。
//! 跑法：cd pinvou3-app/src-tauri && cargo run --example tearoff_spike
//! 预期：移动鼠标 / 按住左键时下面打印的坐标和 down=true 实时变化(跨 3 屏都更新)。
//! Ctrl-C 退出。device_query 在 dev-dependencies，对 examples 可见。

#[cfg(target_os = "linux")]
use device_query::{DeviceQuery, DeviceState, MouseState};
#[cfg(target_os = "linux")]
use std::{thread, time::Duration};

#[cfg(target_os = "linux")]
fn main() {
    let dev = DeviceState::new();
    println!("移动鼠标到不同显示器、按住/松开左键，观察输出。Ctrl-C 退出。");
    let mut last = (i32::MIN, i32::MIN, false);
    loop {
        let m: MouseState = dev.get_mouse();
        let down = *m.button_pressed.get(1).unwrap_or(&false); // index 1 = 左键
        let cur = (m.coords.0, m.coords.1, down);
        if cur != last {
            println!("x={:>5} y={:>5} left_down={}", cur.0, cur.1, down);
            last = cur;
        }
        thread::sleep(Duration::from_millis(16)); // ~60Hz
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("tearoff_spike 仅用于 Linux X11 device_query 验证");
}
