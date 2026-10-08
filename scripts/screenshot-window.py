"""诊断用截图：抓取 DevToolkit 主窗口的真实渲染结果。

抓取方式与坑见 win_capture.py 顶部说明（关键是改用 PrintWindow 以免疫遮挡）。

用法：python screenshot-window.py [输出png] [进程名]
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from win_capture import capture_main_window, make_dpi_aware  # noqa: E402

IMAGE = sys.argv[1] if len(sys.argv) > 1 else r"E:\ai_tools\devtoolkit-app\.shot.png"
PROC = sys.argv[2] if len(sys.argv) > 2 else "devtoolkit.exe"


def main():
    make_dpi_aware()
    try:
        img, mode, meta = capture_main_window(PROC)
    except RuntimeError as e:
        print(str(e))
        return 2
    img.save(IMAGE)
    print("PIDs:", meta["pids"])
    print("WINDOW:", meta["title"], meta["rect"], "hwnd=", meta["hwnd"])
    print("SAVED:", IMAGE, img.size, f"mode={mode}", f"title={meta['title']!r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
