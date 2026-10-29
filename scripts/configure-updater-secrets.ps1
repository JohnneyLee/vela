$ErrorActionPreference = "Stop"

$privateKeyPath = Join-Path $env:USERPROFILE ".tauri\vela-updater.key"
$protectedPasswordPath = Join-Path $env:USERPROFILE ".tauri\vela-updater-password.dpapi"

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
  throw "GitHub CLI (gh) is required. Install it and run 'gh auth login' first."
}
if (-not (Test-Path -LiteralPath $privateKeyPath) -or -not (Test-Path -LiteralPath $protectedPasswordPath)) {
  throw "The local updater signing key or its DPAPI-protected password is missing."
}

$remote = (& git remote get-url origin 2>$null).Trim()
$repoMatch = [regex]::Match($remote, "github\.com[:/]([^/]+/[^/.]+?)(?:\.git)?$")
if (-not $repoMatch.Success) {
  throw "The origin remote is not a GitHub repository."
}
$repository = $repoMatch.Groups[1].Value

function Set-GitHubSecret([string]$name, [string]$value, [string]$repo) {
  $start = [System.Diagnostics.ProcessStartInfo]::new("gh")
  $start.UseShellExecute = $false
  $start.RedirectStandardInput = $true
  $start.RedirectStandardOutput = $true
  $start.RedirectStandardError = $true
  foreach ($argument in @("secret", "set", $name, "--env", "release", "--repo", $repo)) {
    $start.ArgumentList.Add($argument)
  }

  $process = [System.Diagnostics.Process]::Start($start)
  $stdoutTask = $process.StandardOutput.ReadToEndAsync()
  $stderrTask = $process.StandardError.ReadToEndAsync()
  $process.StandardInput.Write($value)
  $process.StandardInput.Close()
  $process.WaitForExit()
  $stdout = $stdoutTask.GetAwaiter().GetResult()
  $stderr = $stderrTask.GetAwaiter().GetResult()
  if ($process.ExitCode -ne 0) {
    throw "Could not configure GitHub Actions secret $name. Check 'gh auth status' and repository permissions. $stderr"
  }
  Write-Output "Configured $name for the release environment in $repo."
}

$encryptedPassword = (Get-Content -Raw -LiteralPath $protectedPasswordPath).Trim()
$securePassword = ConvertTo-SecureString $encryptedPassword
$passwordPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($securePassword)
try {
  $privateKey = Get-Content -Raw -LiteralPath $privateKeyPath
  $password = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($passwordPointer)
  Set-GitHubSecret "TAURI_SIGNING_PRIVATE_KEY" $privateKey $repository
  Set-GitHubSecret "TAURI_SIGNING_PRIVATE_KEY_PASSWORD" $password $repository
} finally {
  $privateKey = $null
  $password = $null
  [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($passwordPointer)
  $securePassword.Dispose()
}
