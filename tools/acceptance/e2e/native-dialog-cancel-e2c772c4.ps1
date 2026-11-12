param([Parameter(Mandatory=$true)][int]$TestProcessId)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class NavkitDialogTest {
  public delegate bool Callback(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(Callback cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
}
'@
$script:found=[IntPtr]::Zero
$until=[DateTime]::UtcNow.AddSeconds(12)
do {
  [void][NavkitDialogTest]::EnumWindows({param($h,$l)
    [uint32]$owner=0
    [void][NavkitDialogTest]::GetWindowThreadProcessId($h,[ref]$owner)
    $cls=New-Object System.Text.StringBuilder 128
    [void][NavkitDialogTest]::GetClassName($h,$cls,128)
    if($owner -eq $TestProcessId -and $cls.ToString() -eq '#32770' -and [NavkitDialogTest]::IsWindowVisible($h)) {$script:found=$h}
    return $true
  },[IntPtr]::Zero)
  if($script:found -ne [IntPtr]::Zero){break}
  Start-Sleep -Milliseconds 100
} while([DateTime]::UtcNow -lt $until)
if($script:found -eq [IntPtr]::Zero){throw 'No native file dialog owned by this test process appeared within 12s'}
[void][NavkitDialogTest]::SetForegroundWindow($script:found)
Start-Sleep -Milliseconds 150
[void][NavkitDialogTest]::PostMessage($script:found,0x100,[IntPtr]27,[IntPtr]0)
[void][NavkitDialogTest]::PostMessage($script:found,0x101,[IntPtr]27,[IntPtr]0)
Start-Sleep -Milliseconds 350
# Native common dialogs dispatch IDCANCEL for Escape; target only the verified owned HWND.
if([NavkitDialogTest]::IsWindowVisible($script:found)) {[void][NavkitDialogTest]::PostMessage($script:found,0x111,[IntPtr]2,[IntPtr]0)}
@{pid=$TestProcessId;hwnd=$script:found.ToInt64();observedNativeDialog=$true;cancel='Escape/IDCANCEL'} | ConvertTo-Json -Compress
