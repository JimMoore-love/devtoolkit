"""抓一张 DevToolkit 主窗口的图，并报告像素统计 —— 用来判断"没画出来"还是"没看到"。

    python scripts/ui-shot.py [输出文件名] [--focus]

--focus 会尝试把窗口提到前台（SetWindowPos TOPMOST + AttachThreadInput），
但这只是为合成点击服务；截图本身走 PrintWindow(PW_RENDERFULLCONTENT)，
按设计不受遮挡影响。若统计显示近纯色，说明渲染进程确实没出内容。
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from win_capture import capture_main_window, force_foreground, release_topmost, list_windows, process_pids  # noqa: E402

name = next((a for a in sys.argv[1:] if not a.startswith('--')), 'ui-now.png')
focus = '--focus' in sys.argv

out = os.path.join('.shots', name)
os.makedirs('.shots', exist_ok=True)

img, mode, meta = capture_main_window()
print(f'窗口        : {meta}')
print(f'抓取方式    : {mode}')

if img is None:
    print('截图失败：PrintWindow 返回空')
    raise SystemExit(1)

wins = list_windows(process_pids('devtoolkit.exe'))
print(f'候选窗口    : {[(w[1], w[2], w[3], w[4], w[5]) for w in wins]}')

if focus and wins:
    hwnd = wins[0][0]
    force_foreground(hwnd)
    print(f'已尝试置前  : {hwnd}')
    import time
    time.sleep(0.8)
    img, mode, meta = capture_main_window()
    print(f'重抓方式    : {mode}')

small = img.resize((64, 64))
px = list(small.getdata())
mean = sum(sum(p) / 3 for p in px) / len(px)
bright = sum(1 for p in px if sum(p) / 3 > 60) / len(px)
uniq = len(set(px))
print(f'尺寸        : {img.size[0]}x{img.size[1]}')
print(f'平均亮度    : {mean:.1f}   (近 0 = 全黑)')
print(f'亮像素占比  : {bright * 100:.1f}%')
print(f'不同颜色数  : {uniq} / {len(px)}')

img.save(out)
print(f'已保存      : {out}')

if focus and wins:
    release_topmost(wins[0][0])
