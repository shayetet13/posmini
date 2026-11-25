@echo off
cd /d "%~dp0"

rem ---- Settings ------------------------------------------------------
rem PORT must match BIND_ADDR in .env
set PORT=8080
rem Tunnel mode:
rem   - no tunnel.yml next to this file = quick tunnel (random https://xxxx.trycloudflare.com link each run)
rem   - tunnel.yml present               = named tunnel with a fixed hostname (see tunnel.example.yml)
rem The global %USERPROFILE%\.cloudflared\config.yml is NOT used, so other tunnels are not affected.
rem --------------------------------------------------------------------

set "PATH=%PATH%;%USERPROFILE%\.cargo\bin;C:\Program Files (x86)\cloudflared;C:\Program Files\cloudflared"
rem Behind the tunnel: use the real client IP (cf-connecting-ip) for the login rate limit
set TRUST_PROXY=true

where cargo >nul 2>nul
if errorlevel 1 (
  echo cargo not found - install Rust first
  pause
  exit /b 1
)
where cloudflared >nul 2>nul
if errorlevel 1 (
  echo cloudflared not found - install with: winget install Cloudflare.cloudflared
  pause
  exit /b 1
)

echo Starting POS server (first run compiles, may take a few minutes)...
start "POS Server" cmd /k "cd /d %~dp0 && cargo run --release"

rem Wait for the server port before opening the tunnel (up to 10 minutes)
set /a tries=0
:wait
powershell -NoProfile -Command "try{(New-Object Net.Sockets.TcpClient('127.0.0.1',%PORT%)).Close();exit 0}catch{exit 1}" >nul 2>nul
if not errorlevel 1 goto up
set /a tries+=1
if %tries% geq 300 (
  echo Server did not start - check the "POS Server" window
  pause
  exit /b 1
)
timeout /t 2 /nobreak >nul
goto wait

:up
echo Server is up. Starting Cloudflare Tunnel...
if exist "%~dp0tunnel.yml" (
  start "Cloudflare Tunnel" cmd /k "cloudflared --config %~dp0tunnel.yml tunnel run"
) else (
  start "Cloudflare Tunnel" cmd /k "cloudflared --config %~dp0tunnel-quick.yml tunnel --url http://127.0.0.1:%PORT%"
  echo.
  echo Find the https://....trycloudflare.com link in the "Cloudflare Tunnel" window,
  echo then paste it in the origin field of the "QR / customer display" button on the cashier page.
)
echo.
echo Cashier on this PC: http://localhost:%PORT%/pos
echo iPad / phone on the same Wi-Fi - open one of these:
powershell -NoProfile -Command "Get-NetIPAddress -AddressFamily IPv4 | Where-Object { $_.IPAddress -notlike '169.254*' -and $_.IPAddress -ne '127.0.0.1' } | ForEach-Object { '    http://' + $_.IPAddress + ':%PORT%/pos' }"
netsh advfirewall firewall show rule name="POS Mini" >nul 2>nul
if errorlevel 1 (
  echo.
  echo If the iPad cannot open the page, allow the port once. Run this in an Administrator terminal:
  echo     netsh advfirewall firewall add rule name="POS Mini" dir=in action=allow protocol=TCP localport=%PORT%
)
echo.
echo To stop: close the "POS Server" and "Cloudflare Tunnel" windows.
timeout /t 20 >nul
