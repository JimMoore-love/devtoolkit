#!/usr/bin/env python
"""MCP 管理页的真机截图工具。

一级导航条目的行位置**不写死**，用墨迹投影自动检测。原因很实际：Read 工具返回的
图片是降采样过的，按它量出来的坐标整体会偏 20% 以上（本项目为此白跑过三轮）。

每次合成点击前都核对目标坐标是否属于本窗口的顶层祖先 —— 不核对就会点穿到
盖在上面的其它应用（本项目真踩过）。

用法：
    python scripts/ui-shot-mcp.py --list                     # 只看导航行位置
    python scripts/ui-shot-mcp.py --click 4 --shot a.png     # 点第 4 项并截图
    python scripts/ui-shot-mcp.py --scroll 6 --after-scroll b.png          # 向下翻页
    python scripts/ui-shot-mcp.py --scroll 6 --click-point 900,700 --after c.png
    python scripts/ui-shot-mcp.py --click-point 640,300 --after b.png   # 点页面内坐标
"""

from __future__ import annotations

import argparse
import ctypes
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import numpy as np  # noqa: E402
from PIL import Image  # noqa: E402

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
MOUSEEVENTF_WHEEL = 0x0800

PROC = "devtoolkit.exe"
# 一级导航（侧栏第一列）文字所在的横向范围
NAV_X0, NAV_X1 = 58, 200
NAV_X = 120


def ink_rows(img, x0, x1, y0, y1, thr=95, minh=4):
    """沿 y 做墨迹投影，返回每一行文字的中心 y。"""
    g = np.asarray(img.convert("L"), dtype=np.int32)[y0:y1, x0:x1]
    on = (g > thr).sum(axis=1) > 1
    rows, start = [], None
    for i, v in enumerate(on):
        if v and start is None:
            start = i
        elif not v and start is not None:
            if i - start >= minh:
                rows.append(y0 + (start + i) // 2)
            start = None
    if start is not None:
        rows.append(y0 + (start + len(on)) // 2)
    return rows


def click(hwnd, left, top, x, y):
    """受控点击：坐标不属于本窗口就中止，绝不盲点。"""
    for attempt in (1, 2):
        _, pid, root = window_at(left + x, top + y)
        if root == hwnd:
            break
        if attempt == 1:
            force_foreground(hwnd)
            time.sleep(0.4)
    else:
        raise SystemExit(f"坐标 ({x},{y}) 不属于目标窗口（root={root}），已中止")

    user32.SetCursorPos(left + x, top + y)
    time.sleep(0.12)
    user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
    time.sleep(0.05)
    user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)


def scroll(hwnd, left, top, x, y, ticks):
    """滚轮翻页。ticks > 0 向下滚，< 0 向上滚。

    滚轮事件投给**光标所在**窗口，所以必须先把光标放进本窗口并核对归属，
    否则会把别人的页面滚走（同 click() 的顾虑）。
    """
    for attempt in (1, 2):
        _, pid, root = window_at(left + x, top + y)
        if root == hwnd:
            break
        if attempt == 1:
            force_foreground(hwnd)
            time.sleep(0.4)
    else:
        raise SystemExit(f"坐标 ({x},{y}) 不属于目标窗口（root={root}），已中止")

    delta = -120 if ticks > 0 else 120
    user32.SetCursorPos(left + x, top + y)
    time.sleep(0.12)
    for _ in range(abs(ticks)):
        user32.mouse_event(MOUSEEVENTF_WHEEL, 0, 0, delta, 0)
        time.sleep(0.06)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--list", action="store_true", help="只列出一级导航行的 y 位置")
    ap.add_argument("--click", type=int, help="点第 N 个导航项（从 1 开始）")
    ap.add_argument("--shot", help="点击导航后截图保存路径")
    ap.add_argument("--scroll", type=int, default=0, help="滚轮步数（正=向下，负=向上）")
    ap.add_argument("--scroll-at", default="800,500", help="滚轮落点（窗口内坐标，默认内容区中部）")
    ap.add_argument("--after-scroll", help="滚动后截图保存路径")
    ap.add_argument("--click-point", help="再点页面内坐标，格式 x,y")
    ap.add_argument("--after", help="再点之后截图保存路径")
    ap.add_argument("--wait", type=float, default=2.5, help="点击后等待秒数")
    a = ap.parse_args()

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
    print(f"WINDOW {title!r} rect=({left},{top},{w},{h}) hwnd={hwnd}")

    prev = user32.GetForegroundWindow()
    force_foreground(hwnd)
    time.sleep(0.6)
    try:
        img = print_window(hwnd, w, h)
        if img is None:
            print("CAPTURE_FAIL")
            return 3

        rows = ink_rows(img, NAV_X0, NAV_X1, 40, h - 20)
        print(f"NAV_ROWS({len(rows)}): {rows}")
        if a.list:
            return 0

        if a.click:
            if not 1 <= a.click <= len(rows):
                print(f"ERROR: 只检测到 {len(rows)} 行，取不到第 {a.click} 项")
                return 4
            y = rows[a.click - 1]
            print(f"点击导航第 {a.click} 项 -> 窗口内 (120,{y})")
            click(hwnd, left, top, NAV_X, y)
            time.sleep(a.wait)
            if a.shot:
                img2 = print_window(hwnd, w, h)
                img2.save(a.shot)
                print("SAVED", a.shot, img2.size)

        if a.scroll:
            sx, sy = (int(v) for v in a.scroll_at.split(","))
            print(f"滚轮 {a.scroll:+d} 步 -> 窗口内 ({sx},{sy})")
            scroll(hwnd, left, top, sx, sy, a.scroll)
            time.sleep(0.6)
            if a.after_scroll:
                img_s = print_window(hwnd, w, h)
                img_s.save(a.after_scroll)
                print("SAVED", a.after_scroll, img_s.size)

        if a.click_point:
            x, y = (int(v) for v in a.click_point.split(","))
            print(f"点击页面内 ({x},{y})")
            click(hwnd, left, top, x, y)
            time.sleep(a.wait)
            if a.after:
                img3 = print_window(hwnd, w, h)
                img3.save(a.after)
                print("SAVED", a.after, img3.size)
    finally:
        release_topmost(hwnd)
        if prev:
            user32.SetForegroundWindow(prev)
    return 0


if __name__ == "__main__":
    sys.exit(main())
