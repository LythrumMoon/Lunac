# Set PATH for cargo and mingw64
$env:PATH = "$env:USERPROFILE\.cargo\bin;D:\cc\claude-code-cli-master\mingw64\bin;$env:PATH"
Set-Location "$PSScriptRoot\..\app"
bun run tauri:dev
