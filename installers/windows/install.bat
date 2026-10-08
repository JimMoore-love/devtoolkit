@echo off
setlocal
REM ============================================================
REM  DevToolkit Windows 安装器（用户级，无需管理员，无 PowerShell/COM 依赖）
REM  双击运行，或命令行：install.bat
REM ============================================================

set "APP=DevToolkit"
set "VER=1.5.0"
set "INSTALLDIR=%LOCALAPPDATA%\Programs\DevToolkit"

echo == %APP% v%VER% 安装 ==

REM 定位源 exe：优先脚本同目录，其次标准 release 路径
set "SRC=%~dp0devtoolkit.exe"
if not exist "%SRC%" set "SRC=%~dp0..\..\src-tauri\target\release\devtoolkit.exe"
if not exist "%SRC%" (
    echo [错误] 未找到 devtoolkit.exe
    echo        请先执行 cargo build --release，或把 exe 放到本脚本同目录。
    goto :end
)

REM 1. 复制程序
if not exist "%INSTALLDIR%" mkdir "%INSTALLDIR%"
copy /Y "%SRC%" "%INSTALLDIR%\devtoolkit.exe" >nul
echo [1/4] 已复制 devtoolkit.exe 到 %INSTALLDIR%

REM 2. 复制卸载脚本
copy /Y "%~dp0uninstall.bat" "%INSTALLDIR%\uninstall.bat" >nul
echo [2/4] 已复制卸载脚本

REM 3. 创建启动器（.cmd，免 COM）
> "%APPDATA%\Microsoft\Windows\Start Menu\Programs\DevToolkit.cmd" echo @start "" "%INSTALLDIR%\devtoolkit.exe"
> "%USERPROFILE%\Desktop\DevToolkit.cmd" echo @start "" "%INSTALLDIR%\devtoolkit.exe"
echo [3/4] 已创建开始菜单 + 桌面启动器

REM 4. 注册卸载入口（HKCU，免管理员）
set "UK=HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\DevToolkit"
reg add "%UK%" /v DisplayName     /t REG_SZ    /d "%APP%" /f >nul
reg add "%UK%" /v DisplayVersion  /t REG_SZ    /d "%VER%" /f >nul
reg add "%UK%" /v Publisher       /t REG_SZ    /d "DevToolkit" /f >nul
reg add "%UK%" /v InstallLocation /t REG_SZ    /d "%INSTALLDIR%" /f >nul
reg add "%UK%" /v DisplayIcon     /t REG_SZ    /d "%INSTALLDIR%\devtoolkit.exe" /f >nul
reg add "%UK%" /v UninstallString /t REG_SZ    /d "\"%INSTALLDIR%\uninstall.bat\"" /f >nul
reg add "%UK%" /v NoModify        /t REG_DWORD /d 1 /f >nul
reg add "%UK%" /v NoRepair        /t REG_DWORD /d 1 /f >nul
echo [4/4] 已注册卸载入口（设置 - 应用 中可卸载）

echo.
echo 安装完成！从开始菜单或桌面启动 DevToolkit。

:end
pause
