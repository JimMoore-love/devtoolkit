@echo off
setlocal
REM ============================================================
REM  DevToolkit Windows 卸载脚本
REM  双击运行，或从「设置 - 应用」卸载入口调用
REM  保留用户配置（项目/历史/杀进程记录），如需彻底清除请手动删除：
REM     %APPDATA%\cn.devtoolkit.app\devtoolkit.json
REM ============================================================

echo == DevToolkit 卸载 ==

REM 1. 删除启动器
del "%APPDATA%\Microsoft\Windows\Start Menu\Programs\DevToolkit.cmd" 2>nul
del "%USERPROFILE%\Desktop\DevToolkit.cmd" 2>nul
echo [1/3] 已删除启动器

REM 2. 删除注册表卸载项
reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\DevToolkit" /f >nul 2>nul
echo [2/3] 已删除卸载注册表项

REM 3. 删除程序目录
rd /s /q "%LOCALAPPDATA%\Programs\DevToolkit" 2>nul
echo [3/3] 已删除程序目录

echo.
echo DevToolkit 已卸载。
echo 提示：用户配置（项目/历史记录）保留在 %%APPDATA%%\cn.devtoolkit.app\，如不需要可手动删除。
pause
