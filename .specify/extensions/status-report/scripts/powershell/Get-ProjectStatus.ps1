#!/usr/bin/env pwsh

# Project status discovery script for /speckit.status.report command
#
# This script discovers project structure and artifact existence.
# It counts task completion and maintains a cache file (specs/spec-status.md)
# so that only feature folders changed since the last cache commit are rescanned.
#
# Usage: ./Get-ProjectStatus.ps1 [OPTIONS]
#
# OPTIONS:
#   -Json               Output in JSON format (default: text)
#   -Feature <name>     Focus on specific feature (name, number, or path)
#   -Help               Show help message

[CmdletBinding()]
param(
    [switch]$Json,
    [string]$Feature,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'

# Define Unicode symbols as escape sequences so the script works
# regardless of file encoding (BOM or no BOM on Windows).
$SYM_CHECK  = [string][char]0x2713  # ✓
$SYM_CIRCLE = [string][char]0x25CB  # ○
$SYM_BULLET = [string][char]0x25CF  # ●

# Show help if requested
if ($Help) {
    Write-Output @"
Usage: Get-ProjectStatus.ps1 [OPTIONS]

Discover project structure and artifact existence for /speckit.status.report.

OPTIONS:
  -Json               Output in JSON format (default: text)
  -Feature <name>     Focus on specific feature (by name, number prefix, or path)
  -Help               Show this help message

EXAMPLES:
  # Get full project status in JSON
  .\Get-ProjectStatus.ps1 -Json

  # Get status for specific feature
  .\Get-ProjectStatus.ps1 -Json -Feature 002-dashboard

  # Get status by feature number
  .\Get-ProjectStatus.ps1 -Json -Feature 002

"@
    exit 0
}

# Function to find repository root
function Find-RepoRoot {
    param([string]$StartPath)

    $dir = $StartPath
    while ($dir -and $dir -ne [System.IO.Path]::GetPathRoot($dir)) {
        if ((Test-Path (Join-Path $dir ".git")) -or (Test-Path (Join-Path $dir ".specify"))) {
            return $dir
        }
        $dir = Split-Path $dir -Parent
    }
    return $null
}

# Function to get project name
function Get-ProjectName {
    param([string]$RepoRoot)

    # Try package.json first
    $packageJson = Join-Path $RepoRoot "package.json"
    if (Test-Path $packageJson) {
        try {
            $pkg = Get-Content $packageJson -Raw | ConvertFrom-Json
            if ($pkg.name) {
                return $pkg.name
            }
        } catch {
            # Ignore parse errors
        }
    }

    # Try pyproject.toml
    $pyproject = Join-Path $RepoRoot "pyproject.toml"
    if (Test-Path $pyproject) {
        $content = Get-Content $pyproject -Raw
        if ($content -match 'name\s*=\s*"([^"]+)"') {
            return $matches[1]
        }
    }

    # Fall back to directory name
    return Split-Path $RepoRoot -Leaf
}

# Function to check if path exists (file or non-empty directory)
function Test-Exists {
    param([string]$Path)

    if (Test-Path $Path -PathType Leaf) {
        return $true
    }
    if ((Test-Path $Path -PathType Container) -and (Get-ChildItem $Path -ErrorAction SilentlyContinue | Select-Object -First 1)) {
        return $true
    }
    return $false
}

# Function to count tasks in a tasks.md file
# Returns hashtable with Total and Completed
function Count-Tasks {
    param([string]$TasksFile)

    $result = @{ Total = 0; Completed = 0 }
    if (Test-Path $TasksFile -PathType Leaf) {
        $lines = Get-Content $TasksFile -ErrorAction SilentlyContinue
        foreach ($line in $lines) {
            if ($line -match '^\s*- \[[ xX]\]') { $result.Total++ }
            if ($line -match '^\s*- \[[xX]\]') { $result.Completed++ }
        }
    }
    return $result
}

# Function to extract a field value from a cache comment line
function Read-CacheField {
    param([string]$Line, [string]$Field)

    if ($Line -match "${Field}=([^ >]+)") {
        return $matches[1]
    }
    return ""
}

# Resolve repository root
$ScriptDir = $PSScriptRoot

try {
    $gitRoot = git rev-parse --show-toplevel 2>$null
    if ($LASTEXITCODE -eq 0) {
        $RepoRoot = $gitRoot
        $HasGit = $true
        $CurrentBranch = git rev-parse --abbrev-ref HEAD 2>$null
        if ($LASTEXITCODE -ne 0) { $CurrentBranch = "unknown" }
    } else {
        throw "Not a git repo"
    }
} catch {
    $RepoRoot = Find-RepoRoot $ScriptDir
    if (-not $RepoRoot) {
        Write-Error "Error: Could not determine repository root."
        exit 1
    }
    $HasGit = $false
    $CurrentBranch = ""
}

# Determine specs directory
$SpecsDir = if (Test-Path (Join-Path $RepoRoot "specs")) {
    Join-Path $RepoRoot "specs"
} elseif (Test-Path (Join-Path $RepoRoot ".specify/specs")) {
    Join-Path $RepoRoot ".specify/specs"
} else {
    Join-Path $RepoRoot "specs"  # Default
}

# Determine memory directory
$MemoryDir = if (Test-Path (Join-Path $RepoRoot ".specify/memory")) {
    Join-Path $RepoRoot ".specify/memory"
} elseif (Test-Path (Join-Path $RepoRoot "memory")) {
    Join-Path $RepoRoot "memory"
} else {
    Join-Path $RepoRoot ".specify/memory"  # Default
}

# Check constitution
$ConstitutionPath = Join-Path $MemoryDir "constitution.md"
$ConstitutionExists = Test-Path $ConstitutionPath -PathType Leaf

# Get project name
$ProjectName = Get-ProjectName $RepoRoot

# Check if on feature branch
$IsFeatureBranch = $CurrentBranch -match '^\d{3}-'

# Collect all features
$Features = @()
if (Test-Path $SpecsDir -PathType Container) {
    $Features = @(Get-ChildItem $SpecsDir -Directory | Where-Object { $_.Name -match '^\d{3}-' } | Sort-Object Name | Select-Object -ExpandProperty Name)
}

# ── Cache setup ───────────────────────────────────────────────────────────────

$CacheFile = Join-Path $SpecsDir "spec-status.md"

# Relative specs path for git commands (forward slashes)
$SpecsRel = $SpecsDir.Replace($RepoRoot, "").TrimStart([IO.Path]::DirectorySeparatorChar).Replace([IO.Path]::DirectorySeparatorChar, "/")

# Find last commit that wrote the cache
$LastCacheCommit = $null
if ($HasGit -and (Test-Path $CacheFile)) {
    $result = git log -1 --format="%H" -- "$SpecsRel/spec-status.md" 2>$null
    if ($LASTEXITCODE -eq 0 -and $result) { $LastCacheCommit = $result }
}

# Determine stale features
$StaleFeatures = @{}
if (-not $LastCacheCommit) {
    # No cache in git history — rescan everything
    foreach ($f in $Features) { $StaleFeatures[$f] = $true }
} else {
    # Collect changed paths since last cache commit, excluding the cache file itself
    $Changed = @()
    $changed1 = git diff --name-only $LastCacheCommit HEAD -- "$SpecsRel/" 2>$null
    $changed2 = git diff --name-only -- "$SpecsRel/" 2>$null
    $changed3 = git diff --cached --name-only -- "$SpecsRel/" 2>$null
    $Changed = @($changed1) + @($changed2) + @($changed3) | Where-Object { $_ -and $_ -notmatch 'spec-status\.md' } | Sort-Object -Unique

    $cacheContent = if (Test-Path $CacheFile) { Get-Content $CacheFile -Raw } else { "" }

    foreach ($f in $Features) {
        $featurePrefix = "$SpecsRel/$f/"
        $hasChanges = $Changed | Where-Object { $_.StartsWith($featurePrefix) }
        $inCache = $cacheContent -match "^<!-- feature: $([regex]::Escape($f)) "
        if ($hasChanges -or -not $inCache) {
            $StaleFeatures[$f] = $true
        }
    }
}

# ── Per-feature data collection ───────────────────────────────────────────────

$FeatureData = @{}
$cacheLines = @{}

# Pre-load cache lines for fresh features
if ($LastCacheCommit -and (Test-Path $CacheFile)) {
    foreach ($line in (Get-Content $CacheFile)) {
        if ($line -match '^<!-- feature: (\S+) ') {
            $cacheLines[$matches[1]] = $line
        }
    }
}

foreach ($f in $Features) {
    $featureDir = Join-Path $SpecsDir $f

    # Determine if current feature
    $isCurrent = $false
    if ($IsFeatureBranch -and $CurrentBranch -match '^(\d{3})-' -and $f -match '^(\d{3})-') {
        $currentPrefix = $CurrentBranch -replace '^(\d{3})-.*', '$1'
        $featurePrefix = $f -replace '^(\d{3})-.*', '$1'
        if ($currentPrefix -eq $featurePrefix) { $isCurrent = $true }
    }

    if ($StaleFeatures.ContainsKey($f)) {
        # ── Fresh scan ────────────────────────────────────────────────────────
        $checklistsDir = Join-Path $featureDir "checklists"
        $hasChecklists = Test-Exists $checklistsDir
        $checklistFiles = @()
        if ($hasChecklists) {
            $checklistFiles = @(Get-ChildItem $checklistsDir -Filter "*.md" -File -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name | Sort-Object)
        }

        $tasks = Count-Tasks (Join-Path $featureDir "tasks.md")

        $FeatureData[$f] = [ordered]@{
            name             = $f
            path             = $featureDir
            is_current       = $isCurrent
            has_spec         = Test-Exists (Join-Path $featureDir "spec.md")
            has_plan         = Test-Exists (Join-Path $featureDir "plan.md")
            has_tasks        = Test-Exists (Join-Path $featureDir "tasks.md")
            has_research     = Test-Exists (Join-Path $featureDir "research.md")
            has_data_model   = Test-Exists (Join-Path $featureDir "data-model.md")
            has_quickstart   = Test-Exists (Join-Path $featureDir "quickstart.md")
            has_contracts    = Test-Exists (Join-Path $featureDir "contracts")
            has_checklists   = $hasChecklists
            checklist_files  = $checklistFiles
            tasks_total      = $tasks.Total
            tasks_completed  = $tasks.Completed
            from_cache       = $false
        }
    } else {
        # ── Load from cache ───────────────────────────────────────────────────
        $line = if ($cacheLines.ContainsKey($f)) { $cacheLines[$f] } else { "" }
        $checklistFilesStr = Read-CacheField $line "checklist_files"
        $checklistFiles = if ($checklistFilesStr) { @($checklistFilesStr -split ',') } else { @() }

        $FeatureData[$f] = [ordered]@{
            name             = $f
            path             = $featureDir
            is_current       = $isCurrent
            has_spec         = (Read-CacheField $line "has_spec") -eq "true"
            has_plan         = (Read-CacheField $line "has_plan") -eq "true"
            has_tasks        = (Read-CacheField $line "has_tasks") -eq "true"
            has_research     = (Read-CacheField $line "has_research") -eq "true"
            has_data_model   = (Read-CacheField $line "has_data_model") -eq "true"
            has_quickstart   = (Read-CacheField $line "has_quickstart") -eq "true"
            has_contracts    = (Read-CacheField $line "has_contracts") -eq "true"
            has_checklists   = (Read-CacheField $line "has_checklists") -eq "true"
            checklist_files  = $checklistFiles
            tasks_total      = [int](Read-CacheField $line "tasks_total")
            tasks_completed  = [int](Read-CacheField $line "tasks_completed")
            from_cache       = $true
        }
    }
}

# ── Write cache file ──────────────────────────────────────────────────────────

function Write-Cache {
    param([string]$CachePath)

    $currentCommit = ""
    if ($HasGit) {
        $currentCommit = git rev-parse HEAD 2>$null
        if ($LASTEXITCODE -ne 0) { $currentCommit = "" }
    }
    $timestamp = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")

    New-Item -ItemType Directory -Path (Split-Path $CachePath) -Force | Out-Null

    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.Add("# Spec-Driven Development Status")
    $lines.Add("<!-- spec-status: project=$ProjectName commit=$currentCommit updated=$timestamp -->")
    $lines.Add("")

    # Determine column width for feature names
    $colWidth = 7  # "Feature"
    foreach ($f in $Features) { if ($f.Length -gt $colWidth) { $colWidth = $f.Length } }

    $headerFeat = "Feature".PadRight($colWidth)
    $lines.Add("| $headerFeat | Specify | Plan | Tasks | Implement |")
    $lines.Add("|$("-" * ($colWidth + 2))|---------|------|-------|-----------|")

    foreach ($f in $Features) {
        $d = $FeatureData[$f]
        $specSym  = if ($d.has_spec)  { $SYM_CHECK } else { "-" }
        $planSym  = if ($d.has_plan)  { $SYM_CHECK } else { "-" }
        $tasksSym = if ($d.has_tasks) { $SYM_CHECK } else { "-" }

        $implementStr = if (-not $d.has_tasks) {
            "-"
        } elseif ($d.tasks_total -eq 0) {
            "$SYM_CIRCLE Ready"
        } elseif ($d.tasks_completed -eq $d.tasks_total) {
            "$SYM_CHECK Complete"
        } else {
            $pct = [int]($d.tasks_completed * 100 / $d.tasks_total)
            "$SYM_BULLET $($d.tasks_completed)/$($d.tasks_total) ($pct%)"
        }

        $featCol = $f.PadRight($colWidth)
        $lines.Add("| $featCol | $($specSym.PadRight(7)) | $($planSym.PadRight(4)) | $($tasksSym.PadRight(5)) | $($implementStr.PadRight(9)) |")
    }

    if ($Features.Count -eq 0) {
        $featCol = "(none)".PadRight($colWidth)
        $lines.Add("| $featCol |         |      |       |           |")
    }

    $lines.Add("")

    # Machine-readable per-feature metadata as HTML comments
    foreach ($f in $Features) {
        $d = $FeatureData[$f]
        $clFiles = if ($d.checklist_files.Count -gt 0) { $d.checklist_files -join ',' } else { "" }
        $boolStr = { param($v) if ($v) { "true" } else { "false" } }
        $lines.Add("<!-- feature: $f has_spec=$(&$boolStr $d.has_spec) has_plan=$(&$boolStr $d.has_plan) has_tasks=$(&$boolStr $d.has_tasks) has_research=$(&$boolStr $d.has_research) has_data_model=$(&$boolStr $d.has_data_model) has_quickstart=$(&$boolStr $d.has_quickstart) has_contracts=$(&$boolStr $d.has_contracts) has_checklists=$(&$boolStr $d.has_checklists) tasks_total=$($d.tasks_total) tasks_completed=$($d.tasks_completed) checklist_files=$clFiles -->")
    }

    Set-Content -Path $CachePath -Value $lines -Encoding UTF8
}

if ((Test-Path $SpecsDir -PathType Container) -or $Features.Count -gt 0) {
    New-Item -ItemType Directory -Path $SpecsDir -Force | Out-Null
    Write-Cache $CacheFile
}

# ── Resolve target feature ────────────────────────────────────────────────────

$ResolvedTarget = $null
if ($Feature) {
    # Try exact match first
    if (Test-Path (Join-Path $SpecsDir $Feature) -PathType Container) {
        $ResolvedTarget = $Feature
    }
    # Try as path
    elseif (Test-Path $Feature -PathType Container) {
        $ResolvedTarget = Split-Path $Feature -Leaf
    }
    # Try as number prefix
    elseif ($Feature -match '^\d+$') {
        $prefix = "{0:D3}" -f [int]$Feature
        $match = $Features | Where-Object { $_ -match "^$prefix-" } | Select-Object -First 1
        if ($match) { $ResolvedTarget = $match }
    }
    # Try partial match
    else {
        $match = $Features | Where-Object { $_ -like "*$Feature*" } | Select-Object -First 1
        if ($match) { $ResolvedTarget = $match }
    }

    if (-not $ResolvedTarget) {
        Write-Error "Error: Feature not found: $Feature"
        exit 1
    }
}

# ── Output results ────────────────────────────────────────────────────────────

if ($Json) {
    $featuresInfo = @()
    foreach ($f in $Features) {
        $featuresInfo += $FeatureData[$f]
    }

    $output = [ordered]@{
        project          = $ProjectName
        repo_root        = $RepoRoot
        specs_dir        = $SpecsDir
        cache_file       = $CacheFile
        has_git          = $HasGit
        branch           = $CurrentBranch
        is_feature_branch = $IsFeatureBranch
        constitution     = [ordered]@{
            exists = $ConstitutionExists
            path   = $ConstitutionPath
        }
        feature_count    = $Features.Count
        target_feature   = $ResolvedTarget
        features         = $featuresInfo
    }

    $output | ConvertTo-Json -Depth 10 -Compress
} else {
    Write-Output "Status Report Discovery"
    Write-Output "========================"
    Write-Output ""
    Write-Output "Project: $ProjectName"
    Write-Output "Root: $RepoRoot"
    Write-Output "Specs: $SpecsDir"
    Write-Output "Cache: $CacheFile"
    Write-Output "Git: $HasGit"
    Write-Output "Branch: $CurrentBranch"
    Write-Output "Feature Branch: $IsFeatureBranch"
    Write-Output "Constitution: $ConstitutionExists ($ConstitutionPath)"
    Write-Output ""

    if ($ResolvedTarget) {
        Write-Output "Target Feature: $ResolvedTarget"
        Write-Output ""
    }

    Write-Output "Features ($($Features.Count)):"
    Write-Output ""

    if ($Features.Count -eq 0) {
        Write-Output "  (none)"
    } else {
        foreach ($f in $Features) {
            $d = $FeatureData[$f]
            Write-Output "  Name: $($d.name)"
            Write-Output "  Path: $($d.path)"
            Write-Output "  Current: $($d.is_current)"
            Write-Output "  From cache: $($d.from_cache)"
            Write-Output "  Artifacts:"
            Write-Output "    spec.md: $($d.has_spec)"
            Write-Output "    plan.md: $($d.has_plan)"
            Write-Output "    tasks.md: $($d.has_tasks)"
            Write-Output "    research.md: $($d.has_research)"
            Write-Output "    data-model.md: $($d.has_data_model)"
            Write-Output "    quickstart.md: $($d.has_quickstart)"
            Write-Output "    contracts/: $($d.has_contracts)"
            Write-Output "    checklists/: $($d.has_checklists)"
            Write-Output "  Tasks: $($d.tasks_completed)/$($d.tasks_total)"
            if ($d.checklist_files.Count -gt 0) {
                Write-Output "    checklist_files: $($d.checklist_files -join ', ')"
            }
            Write-Output ""
        }
    }
}
