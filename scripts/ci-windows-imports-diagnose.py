#!/usr/bin/env python3
"""Windows 测试二进制 PE 导入诊断(定位 STATUS_ENTRYPOINT_NOT_FOUND / 0xc0000139)。

用法:python scripts/ci-windows-imports-diagnose.py [deps 目录]

背景:windows-rust-test 的回归步骤首次真正执行测试 exe 时启动即挂
(exit code 0xc0000139)。该错误是「DLL 找到了但导入的符号不存在」,
Windows 不给出具体缺哪个 DLL/符号,只能静态解析导入表比对导出表。

脚本按 Windows loader 搜索顺序(exe 目录 → System32 → Windows → PATH)
定位每个被导入的 DLL,解析其导出表,报告:
  - 找不到的 DLL(MODULE_NOT_FOUND 类问题)
  - DLL 存在但缺失的导入符号(ENTRYPOINT_NOT_FOUND 类问题)
exe 旁外部清单声明的 SxS 程序集所覆盖的 DLL(如 comctl32)缺符号时
降级为预期提示:loader 按程序集版本解析,静态走文件系统只会看到
System32 副本,其缺 v6 专属符号不构成问题。
仅做诊断,不修改任何文件;exit 0(纯信息输出,不阻断 CI 步骤),
脚本自身异常同样 exit 0 兜底。
"""
import glob
import io
import os
import struct
import sys

# Windows runner 控制台默认 cp1252,打不出中文会 UnicodeEncodeError 崩掉诊断本身
sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')
sys.stderr = io.TextIOWrapper(sys.stderr.buffer, encoding='utf-8', errors='replace')


def read_pe(path):
    data = open(path, 'rb').read()
    pe = struct.unpack_from('<I', data, 0x3C)[0]
    if data[pe:pe + 4] != b'PE\0\0':
        return None
    opt_size = struct.unpack_from('<H', data, pe + 20)[0]
    opt = pe + 24
    magic = struct.unpack_from('<H', data, opt)[0]
    data_dir = opt + (112 if magic == 0x20B else 96)
    nsec = struct.unpack_from('<H', data, pe + 6)[0]
    sections = []
    for i in range(nsec):
        off = opt + opt_size + i * 40
        vsize, va, _rsize, raw = struct.unpack_from('<IIII', data, off + 8)
        sections.append((va, vsize, raw))
    return data, data_dir, sections


def rva_off(rva, sections):
    for va, vsize, raw in sections:
        if va <= rva < va + vsize:
            return raw + (rva - va)
    return None


def imports_of(path):
    """返回 [(dll_name, [imported_symbol, ...]), ...]。"""
    pe = read_pe(path)
    if pe is None:
        return []
    data, data_dir, sections = pe
    imp_rva = struct.unpack_from('<I', data, data_dir + 8)[0]
    if not imp_rva:
        return []
    base = rva_off(imp_rva, sections)
    result = []
    i = 0
    while True:
        oft, _ts, _fc, name_rva, ft = struct.unpack_from('<IIIII', data, base + i * 20)
        if not (oft or name_rva or ft):
            break
        noff = rva_off(name_rva, sections)
        dll = data[noff:data.index(b'\0', noff)].decode('ascii', 'replace')
        funcs = []
        thunk = rva_off(oft or ft, sections)
        j = 0
        while True:
            value = struct.unpack_from('<Q', data, thunk + j * 8)[0]
            if value == 0:
                break
            if value >> 63:  # 按 ordinal 导入
                funcs.append('#%d' % (value & 0xFFFF))
            else:
                hint = rva_off(value, sections)
                end = data.index(b'\0', hint + 2)
                funcs.append(data[hint + 2:end].decode('ascii', 'replace'))
            j += 1
        result.append((dll, funcs))
        i += 1
    return result


def exports_of(path):
    """返回导出符号名集合;无导出表返回空集。"""
    pe = read_pe(path)
    if pe is None:
        return set()
    data, data_dir, sections = pe
    exp_rva = struct.unpack_from('<I', data, data_dir)[0]
    if not exp_rva:
        return set()
    base = rva_off(exp_rva, sections)
    n_names = struct.unpack_from('<I', data, base + 24)[0]
    names_rva = struct.unpack_from('<I', data, base + 32)[0]
    off = rva_off(names_rva, sections)
    out = set()
    for i in range(n_names):
        p = struct.unpack_from('<I', data, off + i * 4)[0]
        o = rva_off(p, sections)
        out.add(data[o:data.index(b'\0', o)].decode('ascii', 'replace'))
    return out


def find_dll(name, exe_dir):
    """按 Windows loader 的简化搜索顺序定位 DLL,返回绝对路径或 None。"""
    candidates = [exe_dir, os.path.join(os.environ.get('SystemRoot', r'C:\Windows'), 'System32'),
                  os.environ.get('SystemRoot', r'C:\Windows')]
    candidates += os.environ.get('PATH', '').split(os.pathsep)
    for d in candidates:
        if not d:
            continue
        p = os.path.join(d, name)
        if os.path.isfile(p):
            return os.path.abspath(p)
    return None


# SxS 程序集名 → 其覆盖的 DLL。loader 见到清单声明的程序集依赖时,绑定程序集
# 版本(如 WinSxS 里的 comctl32 v6)而非 System32 的 v5 副本;脚本静态走文件
# 系统,只会找到 System32 副本,其缺 v6 专属符号(如 TaskDialogIndirect)属预期。
SXS_ASSEMBLY_DLLS = {
    'Microsoft.Windows.Common-Controls': {'comctl32.dll'},
}


def manifest_sxs_dlls(exe):
    """返回 exe 清单声明的 SxS 程序集所覆盖的 DLL 名集合(小写)。

    只读外部清单(<exe>.manifest,Windows loader 在无嵌入清单时的回退);
    嵌入清单在 RT_MANIFEST 资源里,当前不解析。
    """
    mpath = exe + '.manifest'
    if not os.path.isfile(mpath):
        return set()
    try:
        with open(mpath, 'r', encoding='utf-8', errors='replace') as f:
            text = f.read()
    except OSError:
        return set()
    dlls = set()
    for assembly, names in SXS_ASSEMBLY_DLLS.items():
        if assembly in text:
            dlls |= names
    return dlls


def main():
    deps_dir = sys.argv[1] if len(sys.argv) > 1 else '.'
    exes = sorted(glob.glob(os.path.join(deps_dir, 'pinvou3_lib-*.exe')), key=os.path.getmtime)
    if not exes:
        print('DIAG: no pinvou3_lib-*.exe found in', deps_dir)
        return 0
    exe = exes[-1]
    exe_dir = os.path.dirname(exe)
    print('DIAG exe:', exe)
    print('DIAG dlls next to exe:', [f for f in os.listdir(exe_dir) if f.lower().endswith('.dll')])

    problems = []
    sxs_dlls = manifest_sxs_dlls(exe)
    system_dirs = {os.path.normcase(os.path.abspath(p)) for p in (
        os.path.join(os.environ.get('SystemRoot', r'C:\Windows'), 'System32'),
        os.environ.get('SystemRoot', r'C:\Windows'),
    )}
    # exe 直接导入 + 非系统 DLL 的传递递归(loader 递归解析,缺符号可能藏在依赖链)
    queue = [(dll, funcs, exe) for dll, funcs in imports_of(exe)]
    seen = set()
    while queue:
        dll, funcs, importer = queue.pop(0)
        if dll.lower().startswith(('api-ms-', 'ext-ms-')):
            continue
        path = find_dll(dll, os.path.dirname(importer))
        if path is None:
            problems.append(f'缺模块: {dll} (被 {os.path.basename(importer)} 导入,未在 loader 搜索路径找到)')
            continue
        missing = [f for f in funcs if not f.startswith('#') and f not in exports_of(path)]
        indent = '' if importer == exe else '    ↳ '
        if missing and dll.lower() in sxs_dlls:
            print(f'  {indent}{dll} -> {path} ({len(funcs)} imports) '
                  f'缺 {len(missing)} 符号(SxS 清单已声明,loader 按程序集版本解析,属预期)')
        else:
            status = f'缺 {len(missing)} 符号: {missing[:10]}' if missing else 'ok'
            print(f'  {indent}{dll} -> {path} ({len(funcs)} imports) {status}')
            if missing:
                problems.append(f'{path} 缺符号(被 {os.path.basename(importer)} 导入): {missing[:20]}')
        # 非 System32 的 DLL(如 exe 旁拷贝的运行时 DLL)继续传递向下钻
        key = os.path.normcase(path)
        if key not in seen and os.path.normcase(os.path.dirname(path)) not in system_dirs:
            seen.add(key)
            queue.extend((d2, f2, path) for d2, f2 in imports_of(path))

    print()
    if problems:
        print('DIAG 结论(可能即 0xc0000139 根因):')
        for p in problems:
            print('  -', p)
    else:
        print('DIAG 结论: 所有导入 DLL 与符号均可达,0xc0000139 另有原因(delay-load/运行时 LoadLibrary)。')
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except Exception:  # 诊断自身异常只记录,兑现「纯诊断不阻断」的承诺
        import traceback
        traceback.print_exc()
        sys.stderr.write('DIAG: 诊断自身异常,已忽略(不阻断 CI 步骤)\n')
        sys.exit(0)
