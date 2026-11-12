$ErrorActionPreference='Stop'
Get-CimInstance Win32_Process | Where-Object { $_.Name -match 'navkit|mysql|webview|cargo|node' } | ForEach-Object { [pscustomobject]@{Pid=$_.ProcessId;Parent=$_.ParentProcessId;Name=$_.Name;Path=$_.ExecutablePath;CDP=([regex]::Match($_.CommandLine,'--remote-debugging-port=\d+').Value)} } | ConvertTo-Json -Compress
Get-NetTCPConnection -State Listen | Where-Object { $_.LocalPort -in @(3306,9222,9223,9235,1420,4173) } | Select-Object LocalAddress,LocalPort,OwningProcess | ConvertTo-Json -Compress
