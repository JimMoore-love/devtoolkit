"""对照两种抓图方式，判断"页面没画"还是"抓不到"。

PrintWindow(PW_RENDERFULLCONTENT) 是让窗口自己重绘到内存 DC，理论上不受遮挡影响，
但对 WebView2/DirectComposition 并不总可靠；ImageGrab 抓的是屏幕真实像素，一定真，
但要求窗口真的在最上层。

做法：最小化 → 恢复（这一步会强制 WebView2 重新合成），然后两种方式各抓一张对比。
最小化/恢复是可逆的，不改动被观察对象的数据。

    python scripts/ui-diag.py [名前缀]
"""
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from win_capture import (  # noqa: E402
    make_dpi_aware, list_windows, print_window, process_pids, user32,
)

from PIL import ImageGrab  # noqa: E402

SW_MINIMIZE = 6
SW_RESTORE = 9

prefix = sys.argv[1] if len(sys.argv) > 1 else 'diag'
os.makedirs('.shots', exist_ok=True)
make_dpi_aware()


def stats(img, tag):
    if img is None:
        print(f'  {tag}: 空')
        return
    s = img.resize((64, 64))
    px = list(s.getdata())
    mean = sum(sum(p) / 3 for p in px) / len(px)
    bright = sum(1 for p in px if sum(p) / 3 > 60) / len(px)
    dark = sum(1 for p in px if sum(p) / 3 < 40) / len(px)
    print(f'  {tag}: {img.size[0]}x{img.size[1]}  均值 {mean:6.1f}  亮 {bright*100:5.1f}%  暗 {dark*100:5.1f}%  色数 {len(set(px))}')


def grab_all(hwnd, rect, tag):
    l, t, r, b = rect
    img_pw = print_window(hwnd, r - l, b - t)
    img_pw.save(f'.shots/{prefix}-{tag}-printwindow.png')
    img_grab = ImageGrab.grab(bbox=(l, t, r, b))
    img_grab.save(f'.shots/{prefix}-{tag}-imagegrab.png')
    print(f'[{tag}]')
    stats(img_pw, 'PrintWindow ')
    stats(img_grab, 'ImageGrab   ')


wins = list_windows(process_pids('devtoolkit.exe'))
if not wins:
    raise SystemExit('没找到 DevToolkit 窗口')
hwnd, title, l, t, w, h = wins[0]
rect = (l, t, l + w, t + h)
print(f'窗口: {title}  hwnd={hwnd}  rect={rect}')

print('\n--- 当前状态（可能被遮挡） ---')
grab_all(hwnd, rect, 'occluded')

print('\n--- 最小化 → 恢复后 ---')
user32.ShowWindow(hwnd, SW_MINIMIZE)
time.sleep(1.2)
user32.ShowWindow(hwnd, SW_RESTORE)
time.sleep(1.5)
r2 = list_windows(process_pids('devtoolkit.exe'))
if r2:
    hwnd, title, l, t, w, h = r2[0]
    rect = (l, t, l + w, t + h)
    print(f'恢复后 rect={rect}  前台={user32.GetForegroundWindow()}')
grab_all(hwnd, rect, 'restored')

print('\n完成。')
