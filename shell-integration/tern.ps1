# Tern shell integration for PowerShell 7: reports the working directory
# with OSC 7, so splits and restored tabs open where you are,
# and an SSH pane's SFTP sidebar opens in the folder you're in on the server.
# In Tern's local panes it also reports each command you run, for the
# command history suggestions.
# Opt-in. Add to your $PROFILE:  . /path/to/tern.ps1

if (-not $global:__TernPrompt) {
    $global:__TernPrompt = $function:prompt
    $global:__TernHistoryId = $null
    function global:prompt {
        # First, so the original prompt still sees $? of your command.
        $out = & $global:__TernPrompt
        $location = $ExecutionContext.SessionState.Path.CurrentLocation
        if ($location.Provider.Name -eq 'FileSystem') {
            $segments = $location.ProviderPath.Replace('\', '/').Split('/') |
                ForEach-Object { [Uri]::EscapeDataString($_) }
            $path = $segments -join '/'
            if (-not $path.StartsWith('/')) { $path = "/$path" }
            [Console]::Write("`e]7;file://$([Environment]::MachineName)$path`e\")
        }
        if ($env:TERN_HISTORY_TOKEN) {
            $last = Get-History -Count 1
            if ($last -and $last.Id -ne $global:__TernHistoryId) {
                $global:__TernHistoryId = $last.Id
                $command = [Uri]::EscapeDataString($last.CommandLine)
                [Console]::Write("`e]6973;command;$env:TERN_HISTORY_TOKEN;pwsh;$($last.Id);$command`a")
            }
            # A prompt begins, so Tern knows where your input starts.
            [Console]::Write("`e]6973;prompt;$env:TERN_HISTORY_TOKEN;pwsh`a")
        }
        $out
    }
}
