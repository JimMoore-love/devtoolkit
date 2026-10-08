"""真机界面回归：把「网络工具箱」12 个工具面板逐个点开并截图。

用途：改了 UI 之后，不用手点 12 次就能一眼看完所有面板是否渲染正常
（空白、错位、报错提示都能在图上直接看到）。

两道护栏（都是踩过坑才加的）：
  1) 合成鼠标事件发的是屏幕坐标，只要本窗口不是屏幕最上层，点击就会打穿到盖在
     它上面的应用——本项目真踩过（12 次点击全落在一个聊天窗口里）。所以每次点击
     前都用 WindowFromPoint 核对"该坐标的顶层祖先是不是本窗口"，不满足就先抢
     前台重试，再不行立即整体中止，绝不盲点。
  2) 侧栏行位置不再写死手工坐标，改为在截图里做墨迹投影自动检测（分组标题颜色更
     暗，用亮度阈值 + 高度过滤掉）。手工量坐标踩过两次：一次是拿 Read 工具降采样
     后的图去量，整体偏小 21%；一次是应用重启后停在仪表盘，侧栏压根不是那一列。

用法：python ui-walkthrough.py [输出目录]
"""
import os
import sys
import time

import numpy as np
from PIL import ImageChops

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from win_capture import (  # noqa: E402
    force_foreground,
    list_windows,
    make_dpi_aware,
    print_window,
    process_pids,
    release_topmost,
    user32,
    window_at,
)

MOUSEEVENTF_LEFTDOWN = 0x0002
MOUSEEVENTF_LEFTUP = 0x0004

OUT_DIR = sys.argv[1] if len(sys.argv) > 1 else r"E:\ai_tools\devtoolkit-app\.shots"
PROC = "devtoolkit.exe"

# 一级导航「网络工具箱」（最左侧栏）。必须先切过去，否则第二列不是工具列表。
TOP_NAV = ("网络工具箱", 120, 265)

# 第二列工具侧栏：文字标签落在窗口内 x 240..360；点击取该区间中轴
NAV_X = 310
NAV_BAND = (240, 360)  # 检测用的横向范围
NAV_ROWS_TO_SCAN = (100, 700)  # 纵向范围
INK_THRESHOLD = 95  # 分组标题比条目更暗，靠这个阈值滤掉
MIN_BAND_HEIGHT = 8

# 期望检测到的 12 个工具（顺序 = NET_TOOLS 顺序），以及每个面板的等待秒数：
# 等工具挂载 + 首次自动请求返回（ping 8 次探测最久，测速次之）
TOOLS = [
    ("01-ping", 9.0),
    ("02-trace", 6.0),
    ("03-scan", 4.5),
    ("04-dns", 5.0),
    ("05-speed", 12.0),
    ("06-local", 4.5),
    ("07-arp", 6.0),
    ("08-route", 4.5),
    ("09-mac", 4.5),
    ("10-wol", 3.0),
    ("11-calc", 3.0),
    ("12-ref", 3.0),
]


class Abort(Exception):
    """护栏判定不安全，立即停止，不再合成任何点击。"""


def guard(main_hwnd, sx, sy):
    """确认 (sx, sy) 处最上层窗口的顶层祖先就是目标主窗口。"""
    at, pid, root = window_at(sx, sy)
    return root == main_hwnd


def click_safe(main_hwnd, sx, sy):
    if not guard(main_hwnd, sx, sy):
        force_foreground(main_hwnd)
        time.sleep(0.4)
    if not guard(main_hwnd, sx, sy):
        at, pid, root = window_at(sx, sy)
        raise Abort(f"点击坐标 {sx},{sy} 落在 hwnd={at}(pid={pid}, root={root})，不是本窗口，已中止")
    user32.SetCursorPos(sx, sy)
    time.sleep(0.12)
    user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
    time.sleep(0.05)
    user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)


def detect_nav_rows(img):
    """在侧栏标签带做墨迹投影，返回各条目行的纵坐标（窗口内坐标）。"""
    a = np.asarray(img.convert("L"), dtype=np.int32)
    y0, y1 = NAV_ROWS_TO_SCAN
    x0, x1 = NAV_BAND
    ink = (a[y0:y1, x0:x1] > INK_THRESHOLD).sum(axis=1)
    rows, start = [], None
    for i, v in enumerate(ink > 1):
        if v and start is None:
            start = i
        elif not v and start is not None:
            if i - start >= MIN_BAND_HEIGHT:
                rows.append(y0 + (start + i) // 2)
            start = None
    return rows


def main():
    make_dpi_aware()
    pids = process_pids(PROC)
    if not pids:
        print("NO_PROCESS")
        return 2
    wins = list_windows(pids)
    if not wins:
        print("NO_WINDOW")
        return 2
    hwnd, title, left, top, w, h = wins[0]
    print("WINDOW:", title, (left, top, w, h), "hwnd=", hwnd)

    os.makedirs(OUT_DIR, exist_ok=True)
    prev_fg = user32.GetForegroundWindow()
    force_foreground(hwnd)
    time.sleep(0.6)

    try:
        label, tx, ty = TOP_NAV
        click_safe(hwnd, left + tx, top + ty)
        time.sleep(1.5)
        print("SWITCHED TO", label)

        rows = detect_nav_rows(print_window(hwnd, w, h))
        print("检测到侧栏条目 y =", rows)
        if len(rows) != len(TOOLS):
            raise Abort(f"侧栏检测到 {len(rows)} 行，期望 {len(TOOLS)} 行，坐标不可信，已中止")

        prev = None
        for (name, wait), y in zip(TOOLS, rows):
            click_safe(hwnd, left + NAV_X, top + y)
            time.sleep(wait)  # 等挂载 + 首次自动请求返回
            if not guard(hwnd, left + NAV_X, top + y):
                raise Abort(f"{name} 抓图前窗口已被覆盖，结果不可信，已中止")
            img = print_window(hwnd, w, h)
            if img is None:
                print("FAIL", name)
                continue
            # 相邻两张像素完全一致 = 侧栏没点动，必须报出来而不是假装成功
            if prev is not None and ImageChops.difference(prev, img).getbbox() is None:
                print(f"WARN {name} 与上一面板像素完全一致，可能没切换成功")
            prev = img
            img.save(os.path.join(OUT_DIR, f"{name}.png"))
            print(f"SAVED {name} y={y} {img.size} wait={wait:g}s")
    except Abort as e:
        print("ABORT:", e)
        return 3
    finally:
        release_topmost(hwnd)
        if prev_fg:
            user32.SetForegroundWindow(prev_fg)  # 把前台还给用户原来的窗口

    print("DONE ->", OUT_DIR)
    return 0


if __name__ == "__main__":
    sys.exit(main())
