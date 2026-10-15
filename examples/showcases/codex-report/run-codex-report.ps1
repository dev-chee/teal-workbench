$ErrorActionPreference = 'Stop'

$tealPath = 'F:\teal\target\release\teal.exe'
$permissionFile = 'F:\teal\examples\showcases\codex-report\permissions.toml'
$pullArgs = @(
    'run'
    '--tier', 'hosted'
    '--permission-file', $permissionFile
    'F:\teal\examples\showcases\codex-report\codex-report-pull.tl'
)
$agentArgs = @(
    'run'
    '--tier', 'agentic'
    '--permission-file', $permissionFile
    'F:\teal\examples\showcases\codex-report\codex-report.tl'
)

& $tealPath @pullArgs
$pullExitCode = $LASTEXITCODE
if ($pullExitCode -eq 10) {
    exit 0
}
if ($pullExitCode -ne 0) {
    exit $pullExitCode
}

& $tealPath @agentArgs
exit $LASTEXITCODE
