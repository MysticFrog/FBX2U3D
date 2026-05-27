param(
    [Parameter(Mandatory = $true)]
    [string]$InputPath
)

$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Windows.Forms
[void][System.Reflection.Assembly]::LoadWithPartialName('System.Drawing')
[System.Windows.Forms.Application]::EnableVisualStyles()

function Show-QuickConvertMessage {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Message,

        [System.Windows.Forms.MessageBoxIcon]$Icon = [System.Windows.Forms.MessageBoxIcon]::Information
    )

    [System.Windows.Forms.MessageBox]::Show(
        $Message,
        'FBX2U3D Quick Convert',
        [System.Windows.Forms.MessageBoxButtons]::OK,
        $Icon
    ) | Out-Null
}

function Stop-QuickConvertProcess {
    param(
        [Parameter(Mandatory = $true)]
        [System.Diagnostics.Process]$ConvertProcess
    )

    try {
        $taskkillProcess = Start-Process -FilePath (Join-Path $env:SystemRoot 'System32\taskkill.exe') -ArgumentList "/PID $($ConvertProcess.Id) /T /F" -WindowStyle Hidden -PassThru -Wait
        $null = $taskkillProcess
    }
    catch {
        try {
            $ConvertProcess.Kill()
        }
        catch {
        }
    }
}

function Normalize-QuickConvertText {
    param(
        [AllowNull()]
        [string]$Value
    )

    if ([string]::IsNullOrWhiteSpace($Value)) {
        return ''
    }

    return $Value.Trim()
}

$stdoutText = ''
$stderrText = ''
$bannerImage = $null
$process = $null
$script:quickConvertProcess = $null
$script:quickConvertForm = $null
$script:closingForCompletion = $false
$script:conversionCanceled = $false

try {
    $toolPath = Join-Path $PSScriptRoot 'fbx2u3d.exe'
    $converterPath = Join-Path $PSScriptRoot 'u3d-sdk\U3D_A_061228_5\Bin\Win32\Release\IDTFConverter.exe'
    $bannerPath = Join-Path $PSScriptRoot 'FBX2U3D.png'

    if (-not (Test-Path -LiteralPath $toolPath)) {
        throw 'fbx2u3d.exe was not found in the installed application directory.'
    }

    if (-not (Test-Path -LiteralPath $converterPath)) {
        throw 'IDTFConverter.exe was not found in the installed application directory.'
    }

    if (-not (Test-Path -LiteralPath $bannerPath)) {
        throw 'FBX2U3D.png was not found in the installed application directory.'
    }

    $inputFile = (Resolve-Path -LiteralPath $InputPath).Path
    $outputFile = [System.IO.Path]::ChangeExtension($inputFile, '.u3d')

    if (Test-Path -LiteralPath $outputFile) {
        Show-QuickConvertMessage -Message "The output file already exists:`n$outputFile`n`nDelete it or convert manually with --overwrite." -Icon Warning
        exit 1
    }

    $form = New-Object System.Windows.Forms.Form
    $form.Text = 'FBX2U3D Quick Convert'
    $form.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
    $form.ClientSize = New-Object System.Drawing.Size(720, 420)
    $form.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::FixedDialog
    $form.MaximizeBox = $false
    $form.MinimizeBox = $true
    $form.ShowInTaskbar = $true
    $script:quickConvertForm = $form

    $bannerImage = [System.Drawing.Image]::FromFile($bannerPath)
    $horizontalMargin = 16
    $contentWidth = $form.ClientSize.Width - ($horizontalMargin * 2)
    $bannerHeight = [int][Math]::Round(($contentWidth * [double]$bannerImage.Height) / [double][Math]::Max($bannerImage.Width, 1))
    $statusTop = $horizontalMargin + $bannerHeight + 16
    $fileTop = $statusTop + 38
    $pathTop = $fileTop + 28
    $hintTop = $pathTop + 52
    $progressTop = $hintTop + 24
    $form.ClientSize = New-Object System.Drawing.Size(720, ($progressTop + 32))

    $pictureBox = New-Object System.Windows.Forms.PictureBox
    $pictureBox.Location = New-Object System.Drawing.Point($horizontalMargin, $horizontalMargin)
    $pictureBox.Size = New-Object System.Drawing.Size($contentWidth, $bannerHeight)
    $pictureBox.SizeMode = [System.Windows.Forms.PictureBoxSizeMode]::Zoom
    $pictureBox.Image = $bannerImage

    $statusLabel = New-Object System.Windows.Forms.Label
    $statusLabel.Location = New-Object System.Drawing.Point($horizontalMargin, $statusTop)
    $statusLabel.Size = New-Object System.Drawing.Size($contentWidth, 28)
    $statusLabel.Font = New-Object System.Drawing.Font('Segoe UI', 12, [System.Drawing.FontStyle]::Bold)
    $statusLabel.Text = 'Converting FBX to U3D...'

    $fileLabel = New-Object System.Windows.Forms.Label
    $fileLabel.Location = New-Object System.Drawing.Point($horizontalMargin, $fileTop)
    $fileLabel.Size = New-Object System.Drawing.Size($contentWidth, 24)
    $fileLabel.Font = New-Object System.Drawing.Font('Segoe UI', 10, [System.Drawing.FontStyle]::Regular)
    $fileLabel.Text = 'File: ' + [System.IO.Path]::GetFileName($inputFile)

    $pathLabel = New-Object System.Windows.Forms.Label
    $pathLabel.Location = New-Object System.Drawing.Point($horizontalMargin, $pathTop)
    $pathLabel.Size = New-Object System.Drawing.Size($contentWidth, 42)
    $pathLabel.Font = New-Object System.Drawing.Font('Segoe UI', 9, [System.Drawing.FontStyle]::Regular)
    $pathLabel.Text = 'Path: ' + $inputFile

    $hintLabel = New-Object System.Windows.Forms.Label
    $hintLabel.Location = New-Object System.Drawing.Point($horizontalMargin, $hintTop)
    $hintLabel.Size = New-Object System.Drawing.Size($contentWidth, 18)
    $hintLabel.Font = New-Object System.Drawing.Font('Segoe UI', 9, [System.Drawing.FontStyle]::Italic)
    $hintLabel.Text = 'Minimize this window to keep the conversion running in the taskbar. Close it to cancel.'

    $progressBar = New-Object System.Windows.Forms.ProgressBar
    $progressBar.Location = New-Object System.Drawing.Point($horizontalMargin, $progressTop)
    $progressBar.Size = New-Object System.Drawing.Size($contentWidth, 16)
    $progressBar.Style = [System.Windows.Forms.ProgressBarStyle]::Marquee
    $progressBar.MarqueeAnimationSpeed = 30

    $form.Controls.AddRange(@($pictureBox, $statusLabel, $fileLabel, $pathLabel, $hintLabel, $progressBar))

    $quotedInput = '"' + $inputFile.Replace('"', '\"') + '"'
    $quotedOutput = '"' + $outputFile.Replace('"', '\"') + '"'
    $quotedConverter = '"' + $converterPath.Replace('"', '\"') + '"'

    $argumentLine = "$quotedInput --output $quotedOutput --idtf-converter $quotedConverter"
    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = $toolPath
    $startInfo.Arguments = $argumentLine
    $startInfo.WorkingDirectory = $PSScriptRoot
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw 'Quick convert process could not be started.'
    }

    $script:quickConvertProcess = $process

    $form.Add_FormClosing({
        param($formSender, $formClosingEventArgs)

        if ($script:closingForCompletion) {
            return
        }

        if ($null -ne $script:quickConvertProcess -and -not $script:quickConvertProcess.HasExited) {
            $script:conversionCanceled = $true
            $statusLabel.Text = 'Cancelling conversion...'
            $progressBar.MarqueeAnimationSpeed = 0
            Stop-QuickConvertProcess -ConvertProcess $script:quickConvertProcess
        }
    })

    $form.Show()

    while ($form.Visible -and $null -ne $script:quickConvertProcess -and -not $script:quickConvertProcess.HasExited) {
        [System.Windows.Forms.Application]::DoEvents()
        [System.Threading.Thread]::Sleep(100)
    }

    if ($script:conversionCanceled) {
        exit 1
    }

    if ($null -ne $process) {
        $process.WaitForExit()
    }

    $stdoutText = if ($null -ne $process) { Normalize-QuickConvertText -Value $process.StandardOutput.ReadToEnd() } else { '' }
    $stderrText = if ($null -ne $process) { Normalize-QuickConvertText -Value $process.StandardError.ReadToEnd() } else { '' }
    $exitCode = if ($null -ne $process) { $process.ExitCode } else { 1 }
    $convertedSuccessfully = ($exitCode -eq 0) -or (
        (Test-Path -LiteralPath $outputFile) -and
        $stdoutText.StartsWith('Converted ') -and
        [string]::IsNullOrWhiteSpace($stderrText)
    )

    $progressBar.MarqueeAnimationSpeed = 0
    $progressBar.Style = [System.Windows.Forms.ProgressBarStyle]::Continuous
    $progressBar.Value = 100
    $statusLabel.Text = 'Finalizing conversion result...'
    $script:closingForCompletion = $true

    if ($form.Visible) {
        $form.Close()
    }

    [System.Windows.Forms.Application]::DoEvents()

    if ($convertedSuccessfully) {
        Show-QuickConvertMessage -Message "Quick convert completed successfully.`n`n$outputFile"
        exit 0
    }

    $message = @(
        'Quick convert failed.'
        ''
        $stderrText
        $stdoutText
    ) -join "`n"

    Show-QuickConvertMessage -Message (Normalize-QuickConvertText -Value $message) -Icon Error
    exit $exitCode
}
catch {
    Show-QuickConvertMessage -Message ("Quick convert failed.`n`n" + $_.Exception.Message) -Icon Error
    exit 1
}
finally {
    $script:quickConvertProcess = $null
    $script:quickConvertForm = $null
    $script:closingForCompletion = $false
    $script:conversionCanceled = $false

    if ($null -ne $bannerImage) {
        $bannerImage.Dispose()
    }

    if ($null -ne $process) {
        $process.Dispose()
    }
}