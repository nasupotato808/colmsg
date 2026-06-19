@echo off
setlocal
set "COLMSG_UI_CMD=%~f0"
set "COLMSG_UI_ROOT=%~dp0"
powershell.exe -NoProfile -STA -ExecutionPolicy Bypass -Command "$path=$env:COLMSG_UI_CMD; $raw=[System.IO.File]::ReadAllText($path, [System.Text.Encoding]::UTF8); $marker='### POWERSHELL_UI_BELOW ###'; $index=$raw.LastIndexOf($marker); if ($index -lt 0) { throw 'Missing embedded UI script.' }; $script=$raw.Substring($index + $marker.Length).TrimStart([char]13, [char]10); Invoke-Expression $script"
if errorlevel 1 pause
exit /b %errorlevel%
### POWERSHELL_UI_BELOW ###
$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

[System.Windows.Forms.Application]::EnableVisualStyles()

$script:process = $null
if (-not [string]::IsNullOrWhiteSpace($env:COLMSG_UI_ROOT)) {
    $script:root = $env:COLMSG_UI_ROOT
} else {
    $script:root = Split-Path -Parent $MyInvocation.MyCommand.Path
}
$script:errorLogPath = Join-Path $script:root "colmsg-ui-error.log"

function Show-UiError {
    param([string]$Message)

    $timestamp = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
    $entry = "[$timestamp] $Message"
    Add-Content -LiteralPath $script:errorLogPath -Value $entry -Encoding UTF8

    try {
        [System.Windows.Forms.MessageBox]::Show($Message, "colmsg UI error", "OK", "Error") | Out-Null
    } catch {
        Write-Error $Message
    }
}

[System.Windows.Forms.Application]::SetUnhandledExceptionMode(
    [System.Windows.Forms.UnhandledExceptionMode]::CatchException
)

[System.Windows.Forms.Application]::add_ThreadException({
    param($sender, $eventArgs)
    Show-UiError $eventArgs.Exception.ToString()
})

[AppDomain]::CurrentDomain.add_UnhandledException({
    param($sender, $eventArgs)
    Show-UiError $eventArgs.ExceptionObject.ToString()
})

function Resolve-DefaultExePath {
    $releaseExe = Join-Path $script:root "target\release\colmsg.exe"
    $localExe = Join-Path $script:root "colmsg.exe"

    if (Test-Path $releaseExe) { return $releaseExe }
    if (Test-Path $localExe) { return $localExe }
    return $releaseExe
}

function Quote-Argument {
    param([string]$Value)

    if ($null -eq $Value) { return '""' }
    if ($Value -notmatch '[\s"]') { return $Value }

    $escaped = $Value -replace '"', '\"'
    return '"' + $escaped + '"'
}

function Add-Label {
    param(
        [System.Windows.Forms.Control]$Parent,
        [string]$Text,
        [int]$X,
        [int]$Y,
        [int]$Width = 140
    )

    $label = New-Object System.Windows.Forms.Label
    $label.Text = $Text
    $label.Location = New-Object System.Drawing.Point($X, $Y)
    $label.Size = New-Object System.Drawing.Size($Width, 22)
    $label.TextAlign = [System.Drawing.ContentAlignment]::MiddleLeft
    $Parent.Controls.Add($label)
    return $label
}

function Add-TextBox {
    param(
        [System.Windows.Forms.Control]$Parent,
        [int]$X,
        [int]$Y,
        [int]$Width,
        [string]$Text = ""
    )

    $textBox = New-Object System.Windows.Forms.TextBox
    $textBox.Location = New-Object System.Drawing.Point($X, $Y)
    $textBox.Size = New-Object System.Drawing.Size($Width, 24)
    $textBox.Text = $Text
    $Parent.Controls.Add($textBox)
    return $textBox
}

function Add-Button {
    param(
        [System.Windows.Forms.Control]$Parent,
        [string]$Text,
        [int]$X,
        [int]$Y,
        [int]$Width = 90
    )

    $button = New-Object System.Windows.Forms.Button
    $button.Text = $Text
    $button.Location = New-Object System.Drawing.Point($X, $Y)
    $button.Size = New-Object System.Drawing.Size($Width, 28)
    $Parent.Controls.Add($button)
    return $button
}

function Add-LogLine {
    param([string]$Text)

    if ($logBox.InvokeRequired) {
        $null = $logBox.BeginInvoke([Action]{
            $logBox.AppendText($Text + [Environment]::NewLine)
        })
    } else {
        $logBox.AppendText($Text + [Environment]::NewLine)
    }
}

function Set-Status {
    param(
        [string]$Text,
        [bool]$Busy = $false
    )

    if ($statusLabel) {
        $statusLabel.Text = $Text
    }

    if ($progressBar) {
        if ($Busy) {
            $progressBar.Style = [System.Windows.Forms.ProgressBarStyle]::Marquee
        } else {
            $progressBar.Style = [System.Windows.Forms.ProgressBarStyle]::Continuous
            $progressBar.Value = 0
        }
    }
}

function Set-RunningState {
    param([bool]$Running)

    $runButton.Enabled = -not $Running
    $stopButton.Enabled = $Running
    $loadMembersButton.Enabled = -not $Running
    $copyButton.Enabled = -not $Running

    if ($Running) {
        Set-Status "Running..." $true
    }
}

function New-MemberOption {
    param(
        [string]$Name,
        [string]$GroupId,
        [bool]$Subscribed = $false,
        [bool]$IsAll = $false
    )

    $label = $Name
    if ($IsAll) {
        $label = "All subscribed members"
    } elseif (-not [string]::IsNullOrWhiteSpace($GroupId)) {
        $label = "$Name ($GroupId)"
        if ($Subscribed) {
            $label = "$label - subscribed"
        }
    }

    [PSCustomObject]@{
        Label = $label
        Name = $Name
        GroupId = $GroupId
        IsAll = $IsAll
    }
}

function Get-SelectedMemberName {
    $text = $nameBox.Text.Trim()
    if ($nameBox.SelectedItem -and $nameBox.SelectedItem.PSObject.Properties["Name"]) {
        $label = [string]$nameBox.SelectedItem.Label
        if ($text -ne $label) {
            return $text
        }
        return [string]$nameBox.SelectedItem.Name
    }

    return $text
}

function Get-SelectedMemberGroupId {
    if (-not [string]::IsNullOrWhiteSpace($groupIdBox.Text)) {
        return $groupIdBox.Text.Trim()
    }

    if ($nameBox.SelectedItem -and $nameBox.SelectedItem.PSObject.Properties["GroupId"]) {
        $label = [string]$nameBox.SelectedItem.Label
        if ($nameBox.Text.Trim() -ne $label) {
            return ""
        }
        return [string]$nameBox.SelectedItem.GroupId
    }

    return ""
}

function Load-Members {
    $token = $tokenBox.Text.Trim()
    if ([string]::IsNullOrWhiteSpace($token)) {
        throw "Paste a Message API access token first."
    }

    $baseUrl = $baseUrlBox.Text.Trim().TrimEnd("/")
    $headers = @{
        "Authorization" = "Bearer $token"
        "Accept" = "application/json"
        "Content-Type" = "application/json"
        "X-Talk-App-Id" = $appIdBox.Text.Trim()
        "X-Talk-App-Platform" = $platformBox.Text.Trim()
        "Origin" = $originBox.Text.Trim()
        "Referer" = $refererBox.Text.Trim()
    }

    $groups = Invoke-RestMethod -Uri "$baseUrl/v2/groups" -Method Get -Headers $headers
    $members = $groups |
        Where-Object {
            $_.state -eq "open" -and
            -not [string]::IsNullOrWhiteSpace($_.name) -and
            ($_.is_selective_subscription_supported -or $_.subscription)
        } |
        Sort-Object -Property priority -Descending

    $nameBox.BeginUpdate()
    try {
        $nameBox.Items.Clear()
        [void]$nameBox.Items.Add((New-MemberOption "" "" $false $true))

        foreach ($member in $members) {
            [void]$nameBox.Items.Add((New-MemberOption $member.name ([string]$member.id) ($null -ne $member.subscription)))
        }

        $nameBox.SelectedIndex = 0
        $groupIdBox.Clear()
        Add-LogLine ("Loaded " + $members.Count + " members.")
    } finally {
        $nameBox.EndUpdate()
    }
}

function Build-Arguments {
    $arguments = New-Object System.Collections.Generic.List[string]

    $token = $tokenBox.Text.Trim()
    if ([string]::IsNullOrWhiteSpace($token)) {
        throw "Paste a Message API access token first."
    }

    $arguments.Add("--s_access_token")
    $arguments.Add($token)

    if (-not [string]::IsNullOrWhiteSpace($memberIdBox.Text)) {
        $arguments.Add("--member-id")
        $arguments.Add($memberIdBox.Text.Trim())
    }

    $selectedName = Get-SelectedMemberName
    $selectedGroupId = Get-SelectedMemberGroupId

    if (-not [string]::IsNullOrWhiteSpace($selectedGroupId)) {
        $arguments.Add("--message-group-id")
        $arguments.Add($selectedGroupId)
    }

    if (-not [string]::IsNullOrWhiteSpace($selectedName)) {
        $arguments.Add("-n")
        $arguments.Add($selectedName)
    }

    if (-not [string]::IsNullOrWhiteSpace($downloadBox.Text)) {
        $arguments.Add("-d")
        $arguments.Add($downloadBox.Text.Trim())
    }

    if (-not [string]::IsNullOrWhiteSpace($fromBox.Text)) {
        $arguments.Add("-F")
        $arguments.Add($fromBox.Text.Trim())
    }

    $kindChecks = @(
        @{ Box = $kindText; Value = "text" },
        @{ Box = $kindPicture; Value = "picture" },
        @{ Box = $kindVideo; Value = "video" },
        @{ Box = $kindVoice; Value = "voice" },
        @{ Box = $kindLink; Value = "link" }
    )

    $selectedKinds = $kindChecks | Where-Object { $_.Box.Checked }
    if ($selectedKinds.Count -eq 0) {
        throw "Choose at least one message type."
    }

    foreach ($kind in $selectedKinds) {
        $arguments.Add("-k")
        $arguments.Add($kind.Value)
    }

    return $arguments
}

function Build-CommandPreview {
    $exe = $exeBox.Text.Trim()
    $arguments = Build-Arguments
    return (Quote-Argument $exe) + " " + (($arguments | ForEach-Object { Quote-Argument $_ }) -join " ")
}

function Format-ArgumentsForDisplay {
    param([System.Collections.Generic.List[string]]$Arguments)

    $display = New-Object System.Collections.Generic.List[string]
    $redactNext = $false

    foreach ($argument in $Arguments) {
        if ($redactNext) {
            $display.Add("<access-token>")
            $redactNext = $false
            continue
        }

        $display.Add($argument)
        if ($argument -eq "--s_access_token") {
            $redactNext = $true
        }
    }

    return (($display | ForEach-Object { Quote-Argument $_ }) -join " ")
}

function Resolve-DownloadFolder {
    $folder = $downloadBox.Text.Trim()
    if (-not [string]::IsNullOrWhiteSpace($folder)) {
        return $folder
    }

    $exe = $exeBox.Text.Trim()
    if (-not (Test-Path $exe)) {
        throw "Could not find colmsg.exe."
    }

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exe
    $psi.Arguments = "--download-dir"
    $psi.WorkingDirectory = Split-Path -Parent $exe
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    $psi.StandardErrorEncoding = [System.Text.Encoding]::UTF8

    $process = [System.Diagnostics.Process]::Start($psi)
    $output = $process.StandardOutput.ReadToEnd().Trim()
    $process.WaitForExit()

    if ($process.ExitCode -ne 0 -or [string]::IsNullOrWhiteSpace($output)) {
        throw "Could not resolve the default download folder."
    }

    return $output
}

$form = New-Object System.Windows.Forms.Form
$form.Text = "colmsg"
$form.StartPosition = "CenterScreen"
$form.Size = New-Object System.Drawing.Size(820, 760)
$form.MinimumSize = New-Object System.Drawing.Size(760, 680)

$font = New-Object System.Drawing.Font("Segoe UI", 9)
$form.Font = $font

$exeGroup = New-Object System.Windows.Forms.GroupBox
$exeGroup.Text = "Program"
$exeGroup.Location = New-Object System.Drawing.Point(12, 10)
$exeGroup.Size = New-Object System.Drawing.Size(780, 78)
$form.Controls.Add($exeGroup)

Add-Label $exeGroup "colmsg.exe" 12 30 90 | Out-Null
$exeBox = Add-TextBox $exeGroup 110 30 545 (Resolve-DefaultExePath)
$browseExeButton = Add-Button $exeGroup "Browse" 665 28 90

$authGroup = New-Object System.Windows.Forms.GroupBox
$authGroup.Text = "API"
$authGroup.Location = New-Object System.Drawing.Point(12, 96)
$authGroup.Size = New-Object System.Drawing.Size(780, 186)
$form.Controls.Add($authGroup)

Add-Label $authGroup "Access token" 12 28 110 | Out-Null
$tokenBox = Add-TextBox $authGroup 130 28 525
$tokenBox.UseSystemPasswordChar = $true
$showTokenCheck = New-Object System.Windows.Forms.CheckBox
$showTokenCheck.Text = "Show"
$showTokenCheck.Location = New-Object System.Drawing.Point(665, 30)
$showTokenCheck.Size = New-Object System.Drawing.Size(75, 22)
$authGroup.Controls.Add($showTokenCheck)

Add-Label $authGroup "Base URL" 12 62 110 | Out-Null
$baseUrlBox = Add-TextBox $authGroup 130 62 525 "https://api.message.sakurazaka46.com"

Add-Label $authGroup "X-Talk-App-Id" 12 96 110 | Out-Null
$appIdBox = Add-TextBox $authGroup 130 96 525 "jp.co.sonymusic.communication.sakurazaka 2.5"

Add-Label $authGroup "Platform" 12 130 110 | Out-Null
$platformBox = Add-TextBox $authGroup 130 130 160 "web"
Add-Label $authGroup "Origin" 310 130 55 | Out-Null
$originBox = Add-TextBox $authGroup 370 130 285 "https://message.sakurazaka46.com"

Add-Label $authGroup "Referer" 12 160 110 | Out-Null
$refererBox = Add-TextBox $authGroup 130 160 525 "https://message.sakurazaka46.com/"

$targetGroup = New-Object System.Windows.Forms.GroupBox
$targetGroup.Text = "Download Target"
$targetGroup.Location = New-Object System.Drawing.Point(12, 290)
$targetGroup.Size = New-Object System.Drawing.Size(780, 148)
$form.Controls.Add($targetGroup)

Add-Label $targetGroup "Member name" 12 30 110 | Out-Null
$nameBox = New-Object System.Windows.Forms.ComboBox
$nameBox.Location = New-Object System.Drawing.Point(130, 30)
$nameBox.Size = New-Object System.Drawing.Size(280, 24)
$nameBox.DropDownStyle = [System.Windows.Forms.ComboBoxStyle]::DropDown
$nameBox.DisplayMember = "Label"
$nameBox.AutoCompleteMode = [System.Windows.Forms.AutoCompleteMode]::SuggestAppend
$nameBox.AutoCompleteSource = [System.Windows.Forms.AutoCompleteSource]::ListItems
$targetGroup.Controls.Add($nameBox)
[void]$nameBox.Items.Add((New-MemberOption "" "" $false $true))
$nameBox.SelectedIndex = 0
$loadMembersButton = Add-Button $targetGroup "Load" 418 28 70

Add-Label $targetGroup "Member ID" 498 30 75 | Out-Null
$memberIdBox = Add-TextBox $targetGroup 575 30 60

Add-Label $targetGroup "Group ID" 640 30 65 | Out-Null
$groupIdBox = Add-TextBox $targetGroup 705 30 50

Add-Label $targetGroup "Download to" 12 66 110 | Out-Null
$downloadBox = Add-TextBox $targetGroup 130 66 525
$browseFolderButton = Add-Button $targetGroup "Browse" 665 64 90

Add-Label $targetGroup "From" 12 102 110 | Out-Null
$fromBox = Add-TextBox $targetGroup 130 102 180
try {
    $fromBox.PlaceholderText = "YYYY/MM/DD HH:MM:SS"
} catch {
    $fromBox.Text = ""
}

$kindText = New-Object System.Windows.Forms.CheckBox
$kindText.Text = "Text"
$kindText.Checked = $true
$kindText.Location = New-Object System.Drawing.Point(330, 104)
$kindText.Size = New-Object System.Drawing.Size(60, 22)
$targetGroup.Controls.Add($kindText)

$kindPicture = New-Object System.Windows.Forms.CheckBox
$kindPicture.Text = "Picture"
$kindPicture.Checked = $true
$kindPicture.Location = New-Object System.Drawing.Point(395, 104)
$kindPicture.Size = New-Object System.Drawing.Size(70, 22)
$targetGroup.Controls.Add($kindPicture)

$kindVideo = New-Object System.Windows.Forms.CheckBox
$kindVideo.Text = "Video"
$kindVideo.Checked = $true
$kindVideo.Location = New-Object System.Drawing.Point(470, 104)
$kindVideo.Size = New-Object System.Drawing.Size(65, 22)
$targetGroup.Controls.Add($kindVideo)

$kindVoice = New-Object System.Windows.Forms.CheckBox
$kindVoice.Text = "Voice"
$kindVoice.Checked = $true
$kindVoice.Location = New-Object System.Drawing.Point(540, 104)
$kindVoice.Size = New-Object System.Drawing.Size(65, 22)
$targetGroup.Controls.Add($kindVoice)

$kindLink = New-Object System.Windows.Forms.CheckBox
$kindLink.Text = "Link"
$kindLink.Checked = $true
$kindLink.Location = New-Object System.Drawing.Point(610, 104)
$kindLink.Size = New-Object System.Drawing.Size(60, 22)
$targetGroup.Controls.Add($kindLink)

$runButton = Add-Button $form "Run" 22 450 100
$stopButton = Add-Button $form "Stop" 132 450 100
$stopButton.Enabled = $false
$copyButton = Add-Button $form "Copy command" 242 450 120
$openFolderButton = Add-Button $form "Open folder" 372 450 110

$statusLabel = New-Object System.Windows.Forms.Label
$statusLabel.Text = "Ready"
$statusLabel.Location = New-Object System.Drawing.Point(500, 453)
$statusLabel.Size = New-Object System.Drawing.Size(280, 22)
$statusLabel.TextAlign = [System.Drawing.ContentAlignment]::MiddleLeft
$form.Controls.Add($statusLabel)

$progressBar = New-Object System.Windows.Forms.ProgressBar
$progressBar.Location = New-Object System.Drawing.Point(12, 486)
$progressBar.Size = New-Object System.Drawing.Size(780, 12)
$progressBar.Anchor = [System.Windows.Forms.AnchorStyles]::Left -bor
    [System.Windows.Forms.AnchorStyles]::Right -bor
    [System.Windows.Forms.AnchorStyles]::Top
$progressBar.Style = [System.Windows.Forms.ProgressBarStyle]::Continuous
$form.Controls.Add($progressBar)

$logBox = New-Object System.Windows.Forms.TextBox
$logBox.Location = New-Object System.Drawing.Point(12, 506)
$logBox.Size = New-Object System.Drawing.Size(780, 204)
$logBox.Anchor = [System.Windows.Forms.AnchorStyles]::Left -bor
    [System.Windows.Forms.AnchorStyles]::Right -bor
    [System.Windows.Forms.AnchorStyles]::Top -bor
    [System.Windows.Forms.AnchorStyles]::Bottom
$logBox.Multiline = $true
$logBox.ScrollBars = "Vertical"
$logBox.ReadOnly = $true
$form.Controls.Add($logBox)

$browseExeButton.Add_Click({
    $dialog = New-Object System.Windows.Forms.OpenFileDialog
    $dialog.Filter = "colmsg.exe|colmsg.exe|Executable files|*.exe|All files|*.*"
    $dialog.FileName = "colmsg.exe"
    if ($dialog.ShowDialog($form) -eq [System.Windows.Forms.DialogResult]::OK) {
        $exeBox.Text = $dialog.FileName
    }
})

$browseFolderButton.Add_Click({
    $dialog = New-Object System.Windows.Forms.FolderBrowserDialog
    $dialog.SelectedPath = $downloadBox.Text
    if ($dialog.ShowDialog($form) -eq [System.Windows.Forms.DialogResult]::OK) {
        $downloadBox.Text = $dialog.SelectedPath
    }
})

$showTokenCheck.Add_CheckedChanged({
    $tokenBox.UseSystemPasswordChar = -not $showTokenCheck.Checked
})

$nameBox.Add_SelectedIndexChanged({
    if ($nameBox.SelectedItem -and $nameBox.SelectedItem.PSObject.Properties["GroupId"]) {
        $groupId = [string]$nameBox.SelectedItem.GroupId
        $groupIdBox.Text = $groupId
        if (-not [string]::IsNullOrWhiteSpace($groupId)) {
            $memberIdBox.Clear()
        }
    }
})

$nameBox.Add_TextUpdate({
    $groupIdBox.Clear()
    $memberIdBox.Clear()
})

$loadMembersButton.Add_Click({
    try {
        Load-Members
    } catch {
        [System.Windows.Forms.MessageBox]::Show($form, $_.Exception.Message, "colmsg", "OK", "Error") | Out-Null
    }
})

$copyButton.Add_Click({
    try {
        [System.Windows.Forms.Clipboard]::SetText((Build-CommandPreview))
        Add-LogLine "Command copied."
    } catch {
        [System.Windows.Forms.MessageBox]::Show($form, $_.Exception.Message, "colmsg", "OK", "Error") | Out-Null
    }
})

$openFolderButton.Add_Click({
    $folder = Resolve-DownloadFolder
    if (-not (Test-Path $folder)) {
        New-Item -ItemType Directory -Force $folder | Out-Null
    }
    Start-Process explorer.exe $folder
})

$runButton.Add_Click({
    try {
        if ($script:process -ne $null -and -not $script:process.HasExited) {
            return
        }

        $exe = $exeBox.Text.Trim()
        if (-not (Test-Path $exe)) {
            throw "Could not find colmsg.exe."
        }

        $arguments = Build-Arguments
        $argumentText = ($arguments | ForEach-Object { Quote-Argument $_ }) -join " "
        $displayArgumentText = Format-ArgumentsForDisplay $arguments

        $logBox.Clear()
        Add-LogLine "Starting colmsg..."
        Add-LogLine ((Quote-Argument $exe) + " " + $displayArgumentText)
        Add-LogLine "Waiting for download output..."

        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = $exe
        $psi.Arguments = $argumentText
        $psi.WorkingDirectory = Split-Path -Parent $exe
        $psi.UseShellExecute = $false
        $psi.CreateNoWindow = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
        $psi.StandardErrorEncoding = [System.Text.Encoding]::UTF8

        $psi.EnvironmentVariables["S_BASE_URL"] = $baseUrlBox.Text.Trim()
        $psi.EnvironmentVariables["COLMSG_X_TALK_APP_ID"] = $appIdBox.Text.Trim()
        $psi.EnvironmentVariables["COLMSG_X_TALK_APP_PLATFORM"] = $platformBox.Text.Trim()
        $psi.EnvironmentVariables["COLMSG_ORIGIN"] = $originBox.Text.Trim()
        $psi.EnvironmentVariables["COLMSG_REFERER"] = $refererBox.Text.Trim()

        $script:process = New-Object System.Diagnostics.Process
        $script:process.StartInfo = $psi
        $script:process.EnableRaisingEvents = $true

        $script:process.add_OutputDataReceived({
            param($sender, $eventArgs)
            if ($eventArgs.Data) { Add-LogLine $eventArgs.Data }
        })

        $script:process.add_ErrorDataReceived({
            param($sender, $eventArgs)
            if ($eventArgs.Data) { Add-LogLine $eventArgs.Data }
        })

        $script:process.add_Exited({
            param($sender, $eventArgs)
            try {
                $exitCode = $sender.ExitCode
                $null = $form.BeginInvoke([Action]{
                    try {
                        if ($exitCode -eq 0) {
                            Add-LogLine "Complete."
                            Set-Status "Complete" $false
                        } else {
                            Add-LogLine ("Failed. Exit code: " + $exitCode)
                            Set-Status ("Failed. Exit code: " + $exitCode) $false
                        }
                        Set-RunningState $false
                    } catch {
                        Show-UiError $_.Exception.ToString()
                    }
                })
            } catch {
                Show-UiError $_.Exception.ToString()
            }
        })

        Set-RunningState $true
        [void]$script:process.Start()
        $script:process.BeginOutputReadLine()
        $script:process.BeginErrorReadLine()
    } catch {
        Set-RunningState $false
        Set-Status "Ready" $false
        [System.Windows.Forms.MessageBox]::Show($form, $_.Exception.Message, "colmsg", "OK", "Error") | Out-Null
    }
})

$stopButton.Add_Click({
    if ($script:process -ne $null -and -not $script:process.HasExited) {
        $script:process.Kill()
        Add-LogLine "Stopped."
        Set-Status "Stopped" $false
        Set-RunningState $false
    }
})

$form.Add_FormClosing({
    param($sender, $eventArgs)

    if ($script:process -ne $null -and -not $script:process.HasExited) {
        $choice = [System.Windows.Forms.MessageBox]::Show(
            $form,
            "A download is still running. Stop it and close the UI?",
            "colmsg",
            [System.Windows.Forms.MessageBoxButtons]::YesNo,
            [System.Windows.Forms.MessageBoxIcon]::Warning
        )

        if ($choice -ne [System.Windows.Forms.DialogResult]::Yes) {
            $eventArgs.Cancel = $true
            return
        }

        $script:process.Kill()
    }
})

try {
    [void][System.Windows.Forms.Application]::Run($form)
} catch {
    Show-UiError $_.Exception.ToString()
    exit 1
}
