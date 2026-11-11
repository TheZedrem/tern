# Tern shell integration for PowerShell 7: reports the working directory
# with OSC 7, so splits and restored tabs open where you are,
# and an SSH pane's SFTP sidebar opens in the folder you're in on the server.
# Opt-in. Add to your $PROFILE:  . /path/to/tern.ps1

if (-not $global:__TernPrompt) {
    $global:__TernPrompt = $function:prompt
    function global:prompt {
        $location = $ExecutionContext.SessionState.Path.CurrentLocation
        if ($location.Provider.Name -eq 'FileSystem') {
            $segments = $location.ProviderPath.Replace('\', '/').Split('/') |
                ForEach-Object { [Uri]::EscapeDataString($_) }
            $path = $segments -join '/'
            if (-not $path.StartsWith('/')) { $path = "/$path" }
            [Console]::Write("`e]7;file://$([Environment]::MachineName)$path`e\")
        }
        & $global:__TernPrompt
    }
}
