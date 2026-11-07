$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$source = [IO.File]::ReadAllText((Join-Path $root "colmsg-ui.ps1"))
$launcher = [IO.File]::ReadAllText((Join-Path $root "colmsg-ui.cmd"))
$marker = "### POWERSHELL_UI_BELOW ###"
$embedded = $launcher.Substring($launcher.LastIndexOf($marker) + $marker.Length).TrimStart([char]13, [char]10)
if ($embedded -cne $source) { throw "Launcher UI differs from standalone UI" }
$parseErrors = $null
$tokens = $null
$ast = [System.Management.Automation.Language.Parser]::ParseInput($source, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count) { throw ($parseErrors | Out-String) }
$receive = $ast.Find({
    param($node)
    $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq "Receive-ProcessOutput"
}, $true)
Invoke-Expression $receive.Extent.Text
$script:lines = New-Object 'System.Collections.Generic.List[string]'
function Add-LogLine { param([string]$Text) $script:lines.Add($Text) }

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = "powershell.exe"
$psi.Arguments = '-NoProfile -Command "1..251 | ForEach-Object { [Console]::Out.WriteLine($_) }; [Console]::Error.WriteLine(''stderr test'')"'
$psi.UseShellExecute = $false
$psi.CreateNoWindow = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$script:process = New-Object System.Diagnostics.Process
$script:process.StartInfo = $psi
try {
    [void]$script:process.Start()
    $script:stdoutTask = $script:process.StandardOutput.ReadLineAsync()
    $script:stderrTask = $script:process.StandardError.ReadLineAsync()
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        Receive-ProcessOutput
        if ([DateTime]::UtcNow -gt $deadline) { throw "Output drain timed out" }
        Start-Sleep -Milliseconds 10
    } while (-not $script:process.HasExited -or
        $null -ne $script:stdoutTask -or $null -ne $script:stderrTask)
    if ($script:process.ExitCode -ne 0 -or $script:lines.Count -ne 252 -or
        -not $script:lines.Contains("251") -or -not $script:lines.Contains("stderr test")) {
        throw "Process output was lost"
    }
    Write-Output "PASS: UI syntax, embedded launcher, 251 output lines, stderr, and EOF"
} finally {
    if (-not $script:process.HasExited) { $script:process.Kill() }
    $script:process.Dispose()
}
