#!/usr/bin/env python3
"""诊断专用(不合并到 main):chunk 级无缓冲双写。

`cargo test ... | tee file` 里 GNU tee 对「文件」流是 4KB 块缓冲,
runner 突然死亡时文件落后真实输出最多 ~40 行测试进度,无法精确
定位死亡时正在执行的测试。libtest 先打印 `test X ... `(无换行)
再在执行后补 `ok`,行级读取同样会丢掉这个「进行中」的部分行。

本脚本以 4KB chunk 读 stdin,立即写 stdout 与日志文件并各自 flush,
部分行也能实时落盘,配合 ci-live-telemetry.sh 的 tail 即可读到
死亡瞬间真正在跑的测试名。
"""
import os
import sys

LOG_PATH = "/tmp/ci-observe/cargo.log"


def main() -> int:
    os.makedirs(os.path.dirname(LOG_PATH), exist_ok=True)
    src = sys.stdin.buffer
    out = sys.stdout.buffer
    with open(LOG_PATH, "ab", buffering=0) as log:
        while True:
            chunk = src.read1(65536)
            if not chunk:
                return 0
            out.write(chunk)
            out.flush()
            log.write(chunk)


if __name__ == "__main__":
    sys.exit(main())
