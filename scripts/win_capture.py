"""Windows 窗口捕获共用件（供 screenshot-window.py / ui-walkthrough.py 调用）。

为什么不用 ImageGrab.grab(bbox)：
  它抓的是"屏幕该区域此刻的像素"，窗口被别的窗口盖住时拿到的就是别人的内容。
这里用 PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT=2)：由窗口自己重绘到内存 DC，
不受遮挡影响；flag=2 是为了让 WebView2/DirectComposition 的内容也能渲染出来
（flag=0 对 WebView2 常常只得到一片空白）。

只读操作（截屏 / 枚举窗口），不修改被观察对象。
"""
import ctypes
import subprocess
from ctypes import wintypes

from PIL import Image, ImageGrab

user32 = ctypes.windll.user32
gdi32 = ctypes.windll.gdi32
kernel32 = ctypes.windll.kernel32

PW_RENDERFULLCONTENT = 0x00000002
HWND_TOPMOST = -1
HWND_NOTOPMOST = -2
SWP_NOMOVE = 0x0002
SWP_NOSIZE = 0x0001
SWP_NOACTIVATE = 0x0010


class POINT(ctypes.Structure):
    _fields_ = [("x", wintypes.LONG), ("y", wintypes.LONG)]


def make_dpi_aware():
    """高 DPI 下必须先声明感知，否则 GetWindowRect 返回缩放前的逻辑坐标，
    截图会被裁掉右下角、点击坐标也会整体偏移。"""
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)  # PER_MONITOR_AWARE_V2
    except Exception:  # noqa: BLE001
        try:
            user32.SetProcessDPIAware()
        except Exception:  # noqa: BLE001
            pass


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [
        ("biSize", wintypes.DWORD),
        ("biWidth", wintypes.LONG),
        ("biHeight", wintypes.LONG),
        ("biPlanes", wintypes.WORD),
        ("biBitCount", wintypes.WORD),
        ("biCompression", wintypes.DWORD),
        ("biSizeImage", wintypes.DWORD),
        ("biXPelsPerMeter", wintypes.LONG),
        ("biYPelsPerMeter", wintypes.LONG),
        ("biClrUsed", wintypes.DWORD),
        ("biClrImportant", wintypes.DWORD),
    ]


class BITMAPINFO(ctypes.Structure):
    _fields_ = [("bmiHeader", BITMAPINFOHEADER), ("bmiColors", wintypes.DWORD * 3)]


def print_window(hwnd, w, h):
    """让窗口把自己画到内存位图上，返回 PIL RGB Image；失败返回 None。"""
    hdc = user32.GetWindowDC(hwnd)
    if not hdc:
        return None
    memdc = gdi32.CreateCompatibleDC(hdc)
    bmp = gdi32.CreateCompatibleBitmap(hdc, w, h)
    old = gdi32.SelectObject(memdc, bmp)
    try:
        ok = user32.PrintWindow(hwnd, memdc, PW_RENDERFULLCONTENT)
        if not ok:
            ok = user32.PrintWindow(hwnd, memdc, 0)
        if not ok:
            return None

        bi = BITMAPINFO()
        bi.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
        bi.bmiHeader.biWidth = w
        bi.bmiHeader.biHeight = -h  # 负数 = 自上而下，省掉翻转
        bi.bmiHeader.biPlanes = 1
        bi.bmiHeader.biBitCount = 32
        bi.bmiHeader.biCompression = 0  # BI_RGB

        buf = ctypes.create_string_buffer(w * h * 4)
        if not gdi32.GetDIBits(memdc, bmp, 0, h, buf, ctypes.byref(bi), 0):
            return None
        return Image.frombuffer("RGBA", (w, h), buf, "raw", "BGRA", 0, 1).convert("RGB")
    finally:
        gdi32.SelectObject(memdc, old)
        gdi32.DeleteObject(bmp)
        gdi32.DeleteDC(memdc)
        user32.ReleaseDC(hwnd, hdc)


def is_blank(img):
    """全黑/单色说明没拿到内容（WebView2 偶发），需要换法子重试。"""
    if img is None:
        return True
    colors = img.resize((32, 32)).getcolors(32 * 32)
    if not colors:
        return True
    return max(colors)[0] >= 32 * 32 * 0.98


def process_pids(image_name):
    try:
        out = subprocess.check_output(
            ["tasklist", "/FI", f"IMAGENAME eq {image_name}", "/FO", "CSV", "/NH"],
            text=True, encoding="gbk", errors="replace",
        )
    except Exception as e:  # noqa: BLE001
        raise RuntimeError(f"TASKLIST_FAIL {e}") from e
    pids = set()
    for line in out.splitlines():
        parts = [p.strip().strip('"') for p in line.split('","')]
        if len(parts) > 1 and parts[1].isdigit():
            pids.add(int(parts[1]))
    return pids


def list_windows(pids, min_size=200):
    """返回 [(hwnd, title, left, top, w, h)]，按面积从大到小。"""
    found = []
    EnumProc = ctypes.WINFUNCTYPE(ctypes.c_bool, wintypes.HWND, wintypes.LPARAM)

    def on_window(hwnd, _):
        pid = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        if pid.value in pids and user32.IsWindowVisible(hwnd):
            buf = ctypes.create_unicode_buffer(512)
            user32.GetWindowTextW(hwnd, buf, 512)
            rect = wintypes.RECT()
            user32.GetWindowRect(hwnd, ctypes.byref(rect))
            w = rect.right - rect.left
            h = rect.bottom - rect.top
            if w > min_size and h > min_size:
                found.append((hwnd, buf.value, rect.left, rect.top, w, h))
        return True

    user32.EnumWindows(EnumProc(on_window), 0)
    found.sort(key=lambda x: x[4] * x[5], reverse=True)
    return found


def force_foreground(hwnd):
    """把窗口提到最前并尽量拿到焦点。

    直接 SetForegroundWindow 会被 Windows 前台锁拒绝（实测：调用返回后窗口
    仍被别的窗口盖着），于是合成鼠标事件就发到了那个"别的窗口"上。
    这里用两步：先 SetWindowPos(TOPMOST)（不依赖前台权限，纯 z 序）保证它是
    屏幕最上层，再用 AttachThreadInput 打通输入队列后抢前台。
    """
    user32.SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)
    user32.ShowWindow(hwnd, 9)  # SW_RESTORE
    fg = user32.GetForegroundWindow()
    tid_fg = user32.GetWindowThreadProcessId(fg, None) if fg else 0
    tid_me = kernel32.GetCurrentThreadId()
    if tid_fg and tid_fg != tid_me:
        user32.AttachThreadInput(tid_me, tid_fg, True)
        user32.SetForegroundWindow(hwnd)
        user32.BringWindowToTop(hwnd)
        user32.AttachThreadInput(tid_me, tid_fg, False)
    else:
        user32.SetForegroundWindow(hwnd)
        user32.BringWindowToTop(hwnd)


def release_topmost(hwnd):
    """恢复普通 z 序，别把测试窗口永久钉在最上层。"""
    user32.SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)


def window_at(x, y):
    """屏幕坐标 (x, y) 处最上层的窗口，返回 (hwnd, pid, top_level_hwnd)。

    合成点击前必须用它核对，否则会点穿到别的应用上（本项目真踩过：12 次点击
    全落在一个覆盖在上的聊天窗口里）。

    注意不能拿 pid 直接跟目标进程比：Tauri/WebView2 的界面由子进程
    msedgewebview2.exe 渲染，pid 天生不同。要比的是"顶层祖先窗口"
    ——用 GetAncestor(GA_ROOT) 一路问到顶，再跟自己那个 Tauri Window 比。
    """
    hwnd = user32.WindowFromPoint(POINT(x, y))
    if not hwnd:
        return None, None, None
    pid = wintypes.DWORD()
    user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
    root = user32.GetAncestor(hwnd, 2)  # GA_ROOT
    return hwnd, pid.value, root


def capture_main_window(image_name="devtoolkit.exe", fallback_grab=True):
    """定位主窗口并截图，返回 (img, mode, meta)。"""
    pids = process_pids(image_name)
    if not pids:
        raise RuntimeError("NO_PROCESS")
    wins = list_windows(pids)
    if not wins:
        raise RuntimeError("NO_WINDOW")
    hwnd, title, left, top, w, h = wins[0]

    user32.ShowWindow(hwnd, 9)  # SW_RESTORE
    img = print_window(hwnd, w, h)
    mode = "PrintWindow"
    if is_blank(img) and fallback_grab:
        mode = "ImageGrab(fallback)"
        user32.SetForegroundWindow(hwnd)
        import time

        time.sleep(0.6)
        img = ImageGrab.grab(bbox=(left, top, left + w, top + h))
    return img, mode, {"hwnd": hwnd, "title": title, "rect": (left, top, w, h), "pids": sorted(pids)}
