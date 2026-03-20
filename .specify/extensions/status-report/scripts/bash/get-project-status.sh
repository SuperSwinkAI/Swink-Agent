#!/usr/bin/env bash

# Project status discovery script for /speckit.status.report command
#
# This script discovers project structure and artifact existence.
# It counts task completion and maintains a cache file (specs/spec-status.md)
# so that only feature folders changed since the last cache commit are rescanned.
#
# Usage: ./get-project-status.sh [OPTIONS]
#
# OPTIONS:
#   --json              Output in JSON format (default: text)
#   --feature <name>    Focus on specific feature (name, number, or path)
#   --help, -h          Show help message
#
# OUTPUTS:
#   JSON mode: Full project status object (includes tasks_total, tasks_completed, from_cache per feature)
#   Text mode: Human-readable status lines
#   Side effect: Writes/updates {SPECS_DIR}/spec-status.md

set -e

# Parse command line arguments
JSON_MODE=false
TARGET_FEATURE=""

while [ $# -gt 0 ]; do
    case "$1" in
        --json)
            JSON_MODE=true
            shift
            ;;
        --feature)
            if [ -z "$2" ] || [ "${2#--}" != "$2" ]; then
                echo "Error: --feature requires a value" >&2
                exit 1
            fi
            TARGET_FEATURE="$2"
            shift 2
            ;;
        --help|-h)
            cat << 'EOF'
Usage: get-project-status.sh [OPTIONS]

Discover project structure and artifact existence for /speckit.status.report.

OPTIONS:
  --json              Output in JSON format (default: text)
  --feature <name>    Focus on specific feature (by name, number prefix, or path)
  --help, -h          Show this help message

EXAMPLES:
  # Get full project status in JSON
  ./get-project-status.sh --json

  # Get status for specific feature
  ./get-project-status.sh --json --feature 002-dashboard

  # Get status by feature number
  ./get-project-status.sh --json --feature 002

EOF
            exit 0
            ;;
        *)
            # Treat positional arg as feature identifier
            if [ -z "$TARGET_FEATURE" ]; then
                TARGET_FEATURE="$1"
            fi
            shift
            ;;
    esac
done

# Function to find repository root
find_repo_root() {
    local dir="$1"
    while [ "$dir" != "/" ]; do
        if [ -d "$dir/.git" ] || [ -d "$dir/.specify" ]; then
            echo "$dir"
            return 0
        fi
        dir="$(dirname "$dir")"
    done
    return 1
}

# Function to get project name from directory or package.json
get_project_name() {
    local repo_root="$1"

    # Try package.json first
    if [ -f "$repo_root/package.json" ]; then
        local name=$(grep -o '"name"[[:space:]]*:[[:space:]]*"[^"]*"' "$repo_root/package.json" 2>/dev/null | head -1 | sed 's/.*"name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
        if [ -n "$name" ]; then
            echo "$name"
            return
        fi
    fi

    # Try pyproject.toml
    if [ -f "$repo_root/pyproject.toml" ]; then
        local name=$(grep -E '^name[[:space:]]*=' "$repo_root/pyproject.toml" 2>/dev/null | head -1 | sed 's/^name[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/')
        if [ -n "$name" ] && [ "$name" != "$(grep -E '^name[[:space:]]*=' "$repo_root/pyproject.toml" 2>/dev/null | head -1)" ]; then
            echo "$name"
            return
        fi
    fi

    # Fall back to directory name
    basename "$repo_root"
}

# Function to check if path/file exists and is non-empty (for directories)
check_exists() {
    local path="$1"
    if [ -f "$path" ]; then
        echo "true"
    elif [ -d "$path" ] && [ -n "$(ls -A "$path" 2>/dev/null)" ]; then
        echo "true"
    else
        echo "false"
    fi
}

# Function to list files in a directory (for checklists)
list_files() {
    local dir="$1"
    local extension="$2"

    if [ -d "$dir" ]; then
        find "$dir" -maxdepth 1 -name "*$extension" -type f -exec basename {} \; 2>/dev/null | sort
    fi
}

# Function to escape string for JSON
json_escape() {
    local str="$1"
    # Escape backslashes, quotes, and control characters
    printf '%s' "$str" | sed 's/\\/\\\\/g; s/"/\\"/g; s/	/\\t/g' | tr -d '\n\r'
}

# Function to count tasks in a tasks.md file
# Returns: "total completed"
count_tasks() {
    local tasks_file="$1"
    if [ -f "$tasks_file" ]; then
        local total=0
        local completed=0
        total=$(grep -cE '^\s*- \[[ xX]\]' "$tasks_file" 2>/dev/null) || true
        completed=$(grep -cE '^\s*- \[[xX]\]' "$tasks_file" 2>/dev/null) || true
        echo "$total $completed"
    else
        echo "0 0"
    fi
}

# Function to extract a field value from a cache comment line
# Usage: read_cache_field "<!-- feature: name key=val ... -->" "key"
read_cache_field() {
    local line="$1"
    local field="$2"
    echo "$line" | sed -n "s/.*${field}=\([^ >]*\).*/\1/p"
}

# Function to check if a value is in a space-separated list
in_list() {
    local value="$1"
    local list="$2"
    case " $list " in
        *" $value "*) return 0 ;;
        *) return 1 ;;
    esac
}

# Resolve repository root
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

if git rev-parse --show-toplevel >/dev/null 2>&1; then
    REPO_ROOT=$(git rev-parse --show-toplevel)
    HAS_GIT=true
    CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "unknown")
else
    REPO_ROOT="$(find_repo_root "$SCRIPT_DIR")"
    if [ -z "$REPO_ROOT" ]; then
        echo "Error: Could not determine repository root." >&2
        exit 1
    fi
    HAS_GIT=false
    CURRENT_BRANCH=""
fi

# Determine specs directory (specs/ or .specify/specs/)
if [ -d "$REPO_ROOT/specs" ]; then
    SPECS_DIR="$REPO_ROOT/specs"
elif [ -d "$REPO_ROOT/.specify/specs" ]; then
    SPECS_DIR="$REPO_ROOT/.specify/specs"
else
    SPECS_DIR="$REPO_ROOT/specs"  # Default even if doesn't exist
fi

# Determine memory directory (.specify/memory or memory/)
if [ -d "$REPO_ROOT/.specify/memory" ]; then
    MEMORY_DIR="$REPO_ROOT/.specify/memory"
elif [ -d "$REPO_ROOT/memory" ]; then
    MEMORY_DIR="$REPO_ROOT/memory"
else
    MEMORY_DIR="$REPO_ROOT/.specify/memory"  # Default even if doesn't exist
fi

# Check constitution
CONSTITUTION_PATH="$MEMORY_DIR/constitution.md"
CONSTITUTION_EXISTS=$(check_exists "$CONSTITUTION_PATH")

# Get project name
PROJECT_NAME=$(get_project_name "$REPO_ROOT")

# Check if on feature branch (matches NNN-* pattern)
IS_FEATURE_BRANCH=false
if echo "$CURRENT_BRANCH" | grep -qE '^[0-9]{3}-'; then
    IS_FEATURE_BRANCH=true
fi

# Collect all features
FEATURES=()
if [ -d "$SPECS_DIR" ]; then
    for dir in "$SPECS_DIR"/[0-9][0-9][0-9]-*; do
        if [ -d "$dir" ]; then
            FEATURES+=("$(basename "$dir")")
        fi
    done
fi

# Sort features by number
if [ ${#FEATURES[@]} -gt 0 ]; then
    IFS=$'\n' FEATURES=($(sort <<<"${FEATURES[*]}")); unset IFS
fi

# ── Cache setup ───────────────────────────────────────────────────────────────

CACHE_FILE="$SPECS_DIR/spec-status.md"
SPECS_REL="${SPECS_DIR#$REPO_ROOT/}"  # e.g. "specs" or ".specify/specs"

# Find last commit that wrote the cache
LAST_CACHE_COMMIT=""
if [ "$HAS_GIT" = "true" ] && [ -f "$CACHE_FILE" ]; then
    LAST_CACHE_COMMIT=$(git log -1 --format="%H" -- "$SPECS_REL/spec-status.md" 2>/dev/null || true)
fi

# Determine which features need rescanning
STALE_LIST=""
if [ -z "$LAST_CACHE_COMMIT" ]; then
    # No cache in git history — rescan everything
    for feature in "${FEATURES[@]}"; do
        STALE_LIST="$STALE_LIST $feature"
    done
else
    # Find changed paths in specs dir since the cache was last committed,
    # excluding the cache file itself
    CHANGED=$(
        {
            git diff --name-only "$LAST_CACHE_COMMIT" HEAD -- "$SPECS_REL/" 2>/dev/null
            git diff --name-only -- "$SPECS_REL/" 2>/dev/null
            git diff --cached --name-only -- "$SPECS_REL/" 2>/dev/null
        } | grep -v "spec-status\.md" | sort -u
    )

    for feature in "${FEATURES[@]}"; do
        if echo "$CHANGED" | grep -q "^$SPECS_REL/$feature/" || \
           ! grep -q "^<!-- feature: $feature " "$CACHE_FILE" 2>/dev/null; then
            STALE_LIST="$STALE_LIST $feature"
        fi
    done
fi

# ── Per-feature data collection ───────────────────────────────────────────────

# Storage arrays (indexed numerically, parallel to FEATURES array)
FEAT_IS_CURRENT=()
FEAT_HAS_SPEC=()
FEAT_HAS_PLAN=()
FEAT_HAS_TASKS=()
FEAT_HAS_RESEARCH=()
FEAT_HAS_DATA_MODEL=()
FEAT_HAS_QUICKSTART=()
FEAT_HAS_CONTRACTS=()
FEAT_HAS_CHECKLISTS=()
FEAT_CHECKLIST_FILES=()
FEAT_TASKS_TOTAL=()
FEAT_TASKS_COMPLETED=()
FEAT_FROM_CACHE=()

for i in "${!FEATURES[@]}"; do
    feature="${FEATURES[$i]}"
    feature_dir="$SPECS_DIR/$feature"

    # Determine if this is the current feature
    is_current=false
    if [ "$IS_FEATURE_BRANCH" = "true" ]; then
        current_prefix=$(echo "$CURRENT_BRANCH" | grep -o '^[0-9]\{3\}')
        feature_prefix=$(echo "$feature" | grep -o '^[0-9]\{3\}')
        if [ "$current_prefix" = "$feature_prefix" ]; then
            is_current=true
        fi
    fi
    FEAT_IS_CURRENT[$i]="$is_current"

    if in_list "$feature" "$STALE_LIST"; then
        # ── Fresh scan ────────────────────────────────────────────────────────
        FEAT_HAS_SPEC[$i]=$(check_exists "$feature_dir/spec.md")
        FEAT_HAS_PLAN[$i]=$(check_exists "$feature_dir/plan.md")
        FEAT_HAS_TASKS[$i]=$(check_exists "$feature_dir/tasks.md")
        FEAT_HAS_RESEARCH[$i]=$(check_exists "$feature_dir/research.md")
        FEAT_HAS_DATA_MODEL[$i]=$(check_exists "$feature_dir/data-model.md")
        FEAT_HAS_QUICKSTART[$i]=$(check_exists "$feature_dir/quickstart.md")
        FEAT_HAS_CONTRACTS[$i]=$(check_exists "$feature_dir/contracts")
        FEAT_HAS_CHECKLISTS[$i]=$(check_exists "$feature_dir/checklists")

        checklist_files=""
        if [ "${FEAT_HAS_CHECKLISTS[$i]}" = "true" ]; then
            checklist_files=$(list_files "$feature_dir/checklists" ".md" | tr '\n' ',' | sed 's/,$//')
        fi
        FEAT_CHECKLIST_FILES[$i]="$checklist_files"

        task_counts=$(count_tasks "$feature_dir/tasks.md")
        tasks_total=$(echo "$task_counts" | awk '{print $1}')
        tasks_completed=$(echo "$task_counts" | awk '{print $2}')
        FEAT_TASKS_TOTAL[$i]="$tasks_total"
        FEAT_TASKS_COMPLETED[$i]="$tasks_completed"
        FEAT_FROM_CACHE[$i]=false
    else
        # ── Load from cache ───────────────────────────────────────────────────
        cache_line=$(grep "^<!-- feature: $feature " "$CACHE_FILE" 2>/dev/null || true)
        FEAT_HAS_SPEC[$i]=$(read_cache_field "$cache_line" "has_spec")
        FEAT_HAS_PLAN[$i]=$(read_cache_field "$cache_line" "has_plan")
        FEAT_HAS_TASKS[$i]=$(read_cache_field "$cache_line" "has_tasks")
        FEAT_HAS_RESEARCH[$i]=$(read_cache_field "$cache_line" "has_research")
        FEAT_HAS_DATA_MODEL[$i]=$(read_cache_field "$cache_line" "has_data_model")
        FEAT_HAS_QUICKSTART[$i]=$(read_cache_field "$cache_line" "has_quickstart")
        FEAT_HAS_CONTRACTS[$i]=$(read_cache_field "$cache_line" "has_contracts")
        FEAT_HAS_CHECKLISTS[$i]=$(read_cache_field "$cache_line" "has_checklists")
        FEAT_CHECKLIST_FILES[$i]=$(read_cache_field "$cache_line" "checklist_files")
        FEAT_TASKS_TOTAL[$i]=$(read_cache_field "$cache_line" "tasks_total")
        FEAT_TASKS_COMPLETED[$i]=$(read_cache_field "$cache_line" "tasks_completed")
        FEAT_FROM_CACHE[$i]=true
    fi
done

# ── Write cache file ──────────────────────────────────────────────────────────

write_cache() {
    local cache_file="$1"
    local current_commit=""
    if [ "$HAS_GIT" = "true" ]; then
        current_commit=$(git rev-parse HEAD 2>/dev/null || echo "")
    fi
    local timestamp
    timestamp=$(date -u +"%Y-%m-%dT%H:%M:%SZ" 2>/dev/null || date -u)

    # Ensure the specs directory exists
    mkdir -p "$(dirname "$cache_file")"

    {
        echo "# Spec-Driven Development Status"
        echo "<!-- spec-status: project=$(json_escape "$PROJECT_NAME") commit=$current_commit updated=$timestamp -->"
        echo ""

        # Build the markdown table
        local col_widths=7  # minimum "Feature" width
        for feature in "${FEATURES[@]}"; do
            local len=${#feature}
            if [ "$len" -gt "$col_widths" ]; then col_widths=$len; fi
        done

        local header_feat
        printf -v header_feat "%-${col_widths}s" "Feature"
        echo "| $header_feat | Specify | Plan | Tasks | Implement |"
        echo "|$(printf '%0.s-' $(seq 1 $((col_widths + 2))))|---------|------|-------|-----------|"

        for i in "${!FEATURES[@]}"; do
            local feature="${FEATURES[$i]}"
            local specify_sym plan_sym tasks_sym implement_str
            specify_sym=$([ "${FEAT_HAS_SPEC[$i]}" = "true" ] && echo "✓" || echo "-")
            plan_sym=$([ "${FEAT_HAS_PLAN[$i]}" = "true" ] && echo "✓" || echo "-")
            tasks_sym=$([ "${FEAT_HAS_TASKS[$i]}" = "true" ] && echo "✓" || echo "-")

            local total="${FEAT_TASKS_TOTAL[$i]:-0}"
            local completed="${FEAT_TASKS_COMPLETED[$i]:-0}"
            if [ "${FEAT_HAS_TASKS[$i]}" != "true" ]; then
                implement_str="-"
            elif [ "$total" -eq 0 ]; then
                implement_str="○ Ready"
            elif [ "$completed" -eq "$total" ]; then
                implement_str="✓ Complete"
            else
                local pct=$(( completed * 100 / total ))
                implement_str="● $completed/$total ($pct%)"
            fi

            local feat_col
            printf -v feat_col "%-${col_widths}s" "$feature"
            printf "| %s | %-7s | %-4s | %-5s | %-9s |\n" \
                "$feat_col" "$specify_sym" "$plan_sym" "$tasks_sym" "$implement_str"
        done

        if [ "${#FEATURES[@]}" -eq 0 ]; then
            local feat_col
            printf -v feat_col "%-${col_widths}s" "(none)"
            echo "| $feat_col |         |      |       |           |"
        fi

        echo ""

        # Machine-readable per-feature metadata as HTML comments
        for i in "${!FEATURES[@]}"; do
            local feature="${FEATURES[$i]}"
            printf '<!-- feature: %s has_spec=%s has_plan=%s has_tasks=%s has_research=%s has_data_model=%s has_quickstart=%s has_contracts=%s has_checklists=%s tasks_total=%s tasks_completed=%s checklist_files=%s -->\n' \
                "$feature" \
                "${FEAT_HAS_SPEC[$i]}" \
                "${FEAT_HAS_PLAN[$i]}" \
                "${FEAT_HAS_TASKS[$i]}" \
                "${FEAT_HAS_RESEARCH[$i]}" \
                "${FEAT_HAS_DATA_MODEL[$i]}" \
                "${FEAT_HAS_QUICKSTART[$i]}" \
                "${FEAT_HAS_CONTRACTS[$i]}" \
                "${FEAT_HAS_CHECKLISTS[$i]}" \
                "${FEAT_TASKS_TOTAL[$i]:-0}" \
                "${FEAT_TASKS_COMPLETED[$i]:-0}" \
                "${FEAT_CHECKLIST_FILES[$i]}"
        done
    } > "$cache_file"
}

# Only write cache if the specs directory exists (or will exist)
if [ -d "$SPECS_DIR" ] || [ "${#FEATURES[@]}" -gt 0 ]; then
    mkdir -p "$SPECS_DIR"
    write_cache "$CACHE_FILE"
fi

# ── Resolve target feature ────────────────────────────────────────────────────

RESOLVED_TARGET=""
if [ -n "$TARGET_FEATURE" ]; then
    # Try exact match first
    if [ -d "$SPECS_DIR/$TARGET_FEATURE" ]; then
        RESOLVED_TARGET="$TARGET_FEATURE"
    # Try as path
    elif [ -d "$TARGET_FEATURE" ]; then
        RESOLVED_TARGET=$(basename "$TARGET_FEATURE")
    # Try as number prefix
    elif echo "$TARGET_FEATURE" | grep -qE '^[0-9]+$'; then
        PREFIX=$(printf "%03d" "$TARGET_FEATURE")
        for f in "${FEATURES[@]}"; do
            case "$f" in
                "$PREFIX"-*) RESOLVED_TARGET="$f"; break ;;
            esac
        done
    # Try partial match
    else
        for f in "${FEATURES[@]}"; do
            case "$f" in
                *"$TARGET_FEATURE"*) RESOLVED_TARGET="$f"; break ;;
            esac
        done
    fi

    if [ -z "$RESOLVED_TARGET" ]; then
        echo "Error: Feature not found: $TARGET_FEATURE" >&2
        exit 1
    fi
fi

# ── Output results ────────────────────────────────────────────────────────────

# Helper to get feature index by name
get_feature_index() {
    local name="$1"
    for i in "${!FEATURES[@]}"; do
        if [ "${FEATURES[$i]}" = "$name" ]; then
            echo "$i"
            return
        fi
    done
}

get_feature_json() {
    local i="$1"
    local feature="${FEATURES[$i]}"
    local feature_dir="$SPECS_DIR/$feature"
    local checklist_files="${FEAT_CHECKLIST_FILES[$i]}"

    printf '{"name":"%s","path":"%s","is_current":%s,"has_spec":%s,"has_plan":%s,"has_tasks":%s,"has_research":%s,"has_data_model":%s,"has_quickstart":%s,"has_contracts":%s,"has_checklists":%s,"tasks_total":%s,"tasks_completed":%s,"from_cache":%s,"checklist_files":[%s]}' \
        "$(json_escape "$feature")" \
        "$(json_escape "$feature_dir")" \
        "${FEAT_IS_CURRENT[$i]}" \
        "${FEAT_HAS_SPEC[$i]}" \
        "${FEAT_HAS_PLAN[$i]}" \
        "${FEAT_HAS_TASKS[$i]}" \
        "${FEAT_HAS_RESEARCH[$i]}" \
        "${FEAT_HAS_DATA_MODEL[$i]}" \
        "${FEAT_HAS_QUICKSTART[$i]}" \
        "${FEAT_HAS_CONTRACTS[$i]}" \
        "${FEAT_HAS_CHECKLISTS[$i]}" \
        "${FEAT_TASKS_TOTAL[$i]:-0}" \
        "${FEAT_TASKS_COMPLETED[$i]:-0}" \
        "${FEAT_FROM_CACHE[$i]}" \
        "$(if [ -n "$checklist_files" ]; then echo "$checklist_files" | sed 's/\([^,]*\)/"\1"/g'; fi)"
}

if $JSON_MODE; then
    features_json=""
    for i in "${!FEATURES[@]}"; do
        if [ -n "$features_json" ]; then
            features_json="$features_json,"
        fi
        features_json="$features_json$(get_feature_json "$i")"
    done

    printf '{'
    printf '"project":"%s",' "$(json_escape "$PROJECT_NAME")"
    printf '"repo_root":"%s",' "$(json_escape "$REPO_ROOT")"
    printf '"specs_dir":"%s",' "$(json_escape "$SPECS_DIR")"
    printf '"cache_file":"%s",' "$(json_escape "$CACHE_FILE")"
    printf '"has_git":%s,' "$HAS_GIT"
    printf '"branch":"%s",' "$(json_escape "$CURRENT_BRANCH")"
    printf '"is_feature_branch":%s,' "$IS_FEATURE_BRANCH"
    printf '"constitution":{"exists":%s,"path":"%s"},' "$CONSTITUTION_EXISTS" "$(json_escape "$CONSTITUTION_PATH")"
    printf '"feature_count":%d,' "${#FEATURES[@]}"

    if [ -n "$RESOLVED_TARGET" ]; then
        printf '"target_feature":"%s",' "$(json_escape "$RESOLVED_TARGET")"
    else
        printf '"target_feature":null,'
    fi

    printf '"features":[%s]' "$features_json"
    printf '}\n'
else
    echo "Status Report Discovery"
    echo "========================"
    echo ""
    echo "Project: $PROJECT_NAME"
    echo "Root: $REPO_ROOT"
    echo "Specs: $SPECS_DIR"
    echo "Cache: $CACHE_FILE"
    echo "Git: $HAS_GIT"
    echo "Branch: $CURRENT_BRANCH"
    echo "Feature Branch: $IS_FEATURE_BRANCH"
    echo "Constitution: $CONSTITUTION_EXISTS ($CONSTITUTION_PATH)"
    echo ""

    if [ -n "$RESOLVED_TARGET" ]; then
        echo "Target Feature: $RESOLVED_TARGET"
        echo ""
    fi

    echo "Features (${#FEATURES[@]}):"
    echo ""

    if [ ${#FEATURES[@]} -eq 0 ]; then
        echo "  (none)"
    else
        for i in "${!FEATURES[@]}"; do
            feature="${FEATURES[$i]}"
            feature_dir="$SPECS_DIR/$feature"
            echo "  Name: $feature"
            echo "  Path: $feature_dir"
            echo "  Current: ${FEAT_IS_CURRENT[$i]}"
            echo "  From cache: ${FEAT_FROM_CACHE[$i]}"
            echo "  Artifacts:"
            echo "    spec.md: ${FEAT_HAS_SPEC[$i]}"
            echo "    plan.md: ${FEAT_HAS_PLAN[$i]}"
            echo "    tasks.md: ${FEAT_HAS_TASKS[$i]}"
            echo "    research.md: ${FEAT_HAS_RESEARCH[$i]}"
            echo "    data-model.md: ${FEAT_HAS_DATA_MODEL[$i]}"
            echo "    quickstart.md: ${FEAT_HAS_QUICKSTART[$i]}"
            echo "    contracts/: ${FEAT_HAS_CONTRACTS[$i]}"
            echo "    checklists/: ${FEAT_HAS_CHECKLISTS[$i]}"
            echo "  Tasks: ${FEAT_TASKS_COMPLETED[$i]:-0}/${FEAT_TASKS_TOTAL[$i]:-0}"
            if [ -n "${FEAT_CHECKLIST_FILES[$i]}" ]; then
                echo "    checklist_files: ${FEAT_CHECKLIST_FILES[$i]}"
            fi
            echo ""
        done
    fi
fi
