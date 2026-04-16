#!/usr/bin/env bash
# helm-ng asset manager
#
# Downloads, verifies, and manages simulation assets (kernels, binaries,
# firmware) from upstream sources.  Inspired by gem5's resource system.
#
# Usage:
#   scripts/manage-assets.sh download [--all | ID...]   Download resources
#   scripts/manage-assets.sh verify   [--all | ID...]   Verify checksums
#   scripts/manage-assets.sh list     [--category CAT]  List known resources
#   scripts/manage-assets.sh status   [--all | ID...]   Show present/missing
#   scripts/manage-assets.sh clean    [--all | ID...]   Remove downloaded assets
#
# Requires: wget (or curl), sha256sum, tar, jq

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
MANIFEST="$PROJECT_ROOT/scripts/resources.json"
ASSETS_DIR="$PROJECT_ROOT/assets"

# ── color helpers ────────────────────────────────────────────────────────────

if [ -t 1 ]; then
    RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'
    CYAN='\033[0;36m'; BOLD='\033[1m'; RESET='\033[0m'
else
    RED=''; GREEN=''; YELLOW=''; CYAN=''; BOLD=''; RESET=''
fi

info()  { echo -e "${CYAN}[info]${RESET}  $*"; }
ok()    { echo -e "${GREEN}[ok]${RESET}    $*"; }
warn()  { echo -e "${YELLOW}[warn]${RESET}  $*"; }
err()   { echo -e "${RED}[err]${RESET}   $*" >&2; }
die()   { err "$@"; exit 1; }

# ── prerequisites ────────────────────────────────────────────────────────────

check_deps() {
    local missing=()
    for cmd in jq sha256sum; do
        command -v "$cmd" &>/dev/null || missing+=("$cmd")
    done
    if ! command -v wget &>/dev/null && ! command -v curl &>/dev/null; then
        missing+=("wget or curl")
    fi
    if (( ${#missing[@]} )); then
        die "missing required tools: ${missing[*]}"
    fi
}

# ── download helper (prefers wget, falls back to curl) ───────────────────────

fetch_url() {
    local url="$1" dest="$2"
    mkdir -p "$(dirname "$dest")"
    if command -v wget &>/dev/null; then
        wget -q --show-progress -O "$dest" "$url"
    else
        curl -fL --progress-bar -o "$dest" "$url"
    fi
}

# ── manifest queries ─────────────────────────────────────────────────────────

resource_ids() {
    jq -r '.resources[].id' "$MANIFEST"
}

resource_field() {
    local id="$1" field="$2"
    jq -r --arg id "$id" \
        '.resources[] | select(.id == $id) | '"$field" \
        "$MANIFEST"
}

resource_exists() {
    local id="$1"
    jq -e --arg id "$id" '.resources[] | select(.id == $id)' "$MANIFEST" &>/dev/null
}

# ── verify a single file against expected sha256 ─────────────────────────────

verify_sha256() {
    local file="$1" expected="$2"
    if [ ! -f "$file" ]; then
        return 1
    fi
    local actual
    actual="$(sha256sum "$file" | awk '{print $1}')"
    [ "$actual" = "$expected" ]
}

# ── resolve local paths for a resource ───────────────────────────────────────

# Returns the primary file path(s) that indicate this resource is present.
resource_local_paths() {
    local id="$1"
    local src_type
    src_type="$(resource_field "$id" '.source.type')"

    case "$src_type" in
        file)
            echo "$ASSETS_DIR/$(resource_field "$id" '.source.dest')"
            ;;
        archive)
            local extract_to
            extract_to="$(resource_field "$id" '.source.extract_to')"
            # For archives, check if the extract directory exists
            echo "$ASSETS_DIR/$extract_to"
            ;;
        apk)
            local extract_to
            extract_to="$(resource_field "$id" '.source.extract_to')"
            # Return the first listed file as the sentinel
            local first_dest
            first_dest="$(jq -r --arg id "$id" \
                '.resources[] | select(.id == $id) | .source.files | to_entries[0].value.dest' \
                "$MANIFEST")"
            echo "$ASSETS_DIR/$extract_to/$first_dest"
            ;;
    esac
}

# ── download a single resource ───────────────────────────────────────────────

download_one() {
    local id="$1"
    local src_type url
    src_type="$(resource_field "$id" '.source.type')"
    url="$(resource_field "$id" '.source.url')"
    local desc
    desc="$(resource_field "$id" '.description')"

    info "downloading ${BOLD}$id${RESET}: $desc"

    case "$src_type" in
        file)
            local dest="$ASSETS_DIR/$(resource_field "$id" '.source.dest')"
            local sha256
            sha256="$(resource_field "$id" '.source.sha256')"

            if verify_sha256 "$dest" "$sha256"; then
                ok "$id already present and verified"
                return 0
            fi

            fetch_url "$url" "$dest"

            if ! verify_sha256 "$dest" "$sha256"; then
                err "$id: sha256 mismatch after download"
                rm -f "$dest"
                return 1
            fi

            # Post-install hook (e.g. create symlinks)
            local post
            post="$(resource_field "$id" '.source.post_install // empty')"
            if [ -n "$post" ]; then
                eval "$post"
            fi

            # Make ELF files executable
            if file "$dest" 2>/dev/null | grep -q 'ELF'; then
                chmod +x "$dest"
            fi

            ok "$id downloaded and verified"
            ;;

        archive)
            local extract_to="$ASSETS_DIR/$(resource_field "$id" '.source.extract_to')"
            local sha256
            sha256="$(resource_field "$id" '.source.sha256')"
            local tmp_archive
            tmp_archive="$(mktemp)"
            trap "rm -f '$tmp_archive'" RETURN

            fetch_url "$url" "$tmp_archive"

            if ! verify_sha256 "$tmp_archive" "$sha256"; then
                err "$id: archive sha256 mismatch"
                rm -f "$tmp_archive"
                return 1
            fi

            mkdir -p "$extract_to"

            # Detect archive type and extract
            case "$url" in
                *.tar.gz|*.tgz)   tar -xzf "$tmp_archive" -C "$extract_to" ;;
                *.tar.xz)         tar -xJf "$tmp_archive" -C "$extract_to" ;;
                *.tar.bz2)        tar -xjf "$tmp_archive" -C "$extract_to" ;;
                *.tar)            tar -xf  "$tmp_archive" -C "$extract_to" ;;
                *)                die "unknown archive format: $url" ;;
            esac

            # Handle extract_files mapping (rename specific files)
            local extract_files
            extract_files="$(resource_field "$id" '.source.extract_files // empty')"
            if [ -n "$extract_files" ] && [ "$extract_files" != "null" ]; then
                echo "$extract_files" | jq -r 'to_entries[] | "\(.key)\t\(.value)"' | \
                while IFS=$'\t' read -r src dst; do
                    if [ -f "$extract_to/$src" ]; then
                        mv "$extract_to/$src" "$extract_to/$dst"
                    fi
                done
            fi

            # Optionally keep the archive file alongside extracted content
            local keep
            keep="$(resource_field "$id" '.source.keep_archive // false')"
            if [ "$keep" = "true" ]; then
                local archive_name
                archive_name="$(basename "$url")"
                mv "$tmp_archive" "$extract_to/$archive_name"
            else
                rm -f "$tmp_archive"
            fi

            ok "$id extracted to $extract_to"
            ;;

        apk)
            local extract_to="$ASSETS_DIR/$(resource_field "$id" '.source.extract_to')"
            local tmp_apk
            tmp_apk="$(mktemp)"
            trap "rm -f '$tmp_apk'" RETURN

            fetch_url "$url" "$tmp_apk"

            # APK files are gzipped tar archives; extract to a temp dir
            # then pull out the specific files we need
            local tmp_dir
            tmp_dir="$(mktemp -d)"

            # APK v2 format: gzipped tar with a data.tar.gz inside,
            # or sometimes the files are directly in the tar.
            # Alpine APKs are gzipped tarballs with files directly inside.
            tar -xzf "$tmp_apk" -C "$tmp_dir" 2>/dev/null || true

            mkdir -p "$extract_to"

            # Extract the specified files
            local files_json
            files_json="$(jq --arg id "$id" \
                '.resources[] | select(.id == $id) | .source.files' \
                "$MANIFEST")"

            local all_ok=true
            echo "$files_json" | jq -r 'to_entries[] | "\(.key)\t\(.value.dest)\t\(.value.sha256)"' | \
            while IFS=$'\t' read -r src dest expected_sha; do
                local src_path="$tmp_dir/$src"
                local dest_path="$extract_to/$dest"

                if [ ! -f "$src_path" ]; then
                    # Try without the leading directory
                    src_path="$(find "$tmp_dir" -name "$(basename "$src")" -type f | head -1)"
                fi

                if [ -z "$src_path" ] || [ ! -f "$src_path" ]; then
                    err "$id: file '$src' not found in APK"
                    all_ok=false
                    continue
                fi

                cp "$src_path" "$dest_path"

                if ! verify_sha256 "$dest_path" "$expected_sha"; then
                    warn "$id: sha256 mismatch for $dest (file may have been updated upstream)"
                fi
            done

            # Also extract dtbs if present
            if [ -d "$tmp_dir/boot/dtbs-lts" ]; then
                cp -r "$tmp_dir/boot/dtbs-lts" "$extract_to/"
            fi

            rm -rf "$tmp_dir" "$tmp_apk"

            ok "$id extracted to $extract_to"
            ;;
    esac
}

# ── verify a single resource ─────────────────────────────────────────────────

verify_one() {
    local id="$1"
    local src_type
    src_type="$(resource_field "$id" '.source.type')"

    case "$src_type" in
        file)
            local dest="$ASSETS_DIR/$(resource_field "$id" '.source.dest')"
            local sha256
            sha256="$(resource_field "$id" '.source.sha256')"
            if [ ! -f "$dest" ]; then
                warn "$id: ${BOLD}MISSING${RESET} ($dest)"
                return 1
            fi
            if verify_sha256 "$dest" "$sha256"; then
                ok "$id: verified"
            else
                err "$id: ${BOLD}CHECKSUM MISMATCH${RESET} ($dest)"
                return 1
            fi
            ;;
        archive)
            local sha256
            sha256="$(resource_field "$id" '.source.sha256 // empty')"
            local extract_to="$ASSETS_DIR/$(resource_field "$id" '.source.extract_to')"
            if [ ! -d "$extract_to" ]; then
                warn "$id: ${BOLD}MISSING${RESET} ($extract_to)"
                return 1
            fi
            ok "$id: directory present ($extract_to)"
            ;;
        apk)
            local extract_to="$ASSETS_DIR/$(resource_field "$id" '.source.extract_to')"
            local files_json
            files_json="$(jq --arg id "$id" \
                '.resources[] | select(.id == $id) | .source.files' \
                "$MANIFEST")"
            local all_ok=true
            while IFS=$'\t' read -r _ dest expected_sha; do
                local dest_path="$extract_to/$dest"
                if [ ! -f "$dest_path" ]; then
                    warn "$id/$dest: ${BOLD}MISSING${RESET}"
                    all_ok=false
                elif ! verify_sha256 "$dest_path" "$expected_sha"; then
                    warn "$id/$dest: ${BOLD}CHECKSUM CHANGED${RESET}"
                    all_ok=false
                fi
            done < <(echo "$files_json" | jq -r 'to_entries[] | "\(.key)\t\(.value.dest)\t\(.value.sha256)"')
            if $all_ok; then
                ok "$id: all files verified"
            else
                return 1
            fi
            ;;
    esac
}

# ── status of a single resource ──────────────────────────────────────────────

status_one() {
    local id="$1"
    local desc arch cat
    desc="$(resource_field "$id" '.description')"
    arch="$(resource_field "$id" '.architecture')"
    cat="$(resource_field "$id" '.category')"
    local path
    path="$(resource_local_paths "$id")"

    local state
    if [ -e "$path" ]; then
        state="${GREEN}present${RESET}"
    else
        state="${RED}missing${RESET}"
    fi

    printf "  %-28s %-10s %-10s %b  %s\n" "$id" "$arch" "$cat" "$state" "$desc"
}

# ── clean a single resource ──────────────────────────────────────────────────

clean_one() {
    local id="$1"
    local src_type
    src_type="$(resource_field "$id" '.source.type')"

    case "$src_type" in
        file)
            local dest="$ASSETS_DIR/$(resource_field "$id" '.source.dest')"
            if [ -f "$dest" ]; then
                rm -f "$dest"
                ok "$id: removed $dest"
            else
                info "$id: already absent"
            fi
            ;;
        archive|apk)
            local extract_to="$ASSETS_DIR/$(resource_field "$id" '.source.extract_to')"
            if [ -d "$extract_to" ]; then
                rm -rf "$extract_to"
                ok "$id: removed $extract_to"
            else
                info "$id: already absent"
            fi
            ;;
    esac
}

# ── resolve ID list from args ────────────────────────────────────────────────

resolve_ids() {
    local args=("$@")
    if (( ${#args[@]} == 0 )) || [ "${args[0]}" = "--all" ]; then
        resource_ids
    else
        for id in "${args[@]}"; do
            if ! resource_exists "$id"; then
                die "unknown resource: $id (use 'list' to see available)"
            fi
            echo "$id"
        done
    fi
}

# ── commands ─────────────────────────────────────────────────────────────────

cmd_download() {
    local ids
    ids="$(resolve_ids "$@")"
    local fail=0
    while IFS= read -r id; do
        download_one "$id" || (( fail++ )) || true
    done <<< "$ids"
    echo
    if (( fail > 0 )); then
        err "$fail resource(s) failed"
        return 1
    fi
    ok "all requested resources downloaded"
}

cmd_verify() {
    local ids
    ids="$(resolve_ids "$@")"
    local fail=0
    while IFS= read -r id; do
        verify_one "$id" || (( fail++ )) || true
    done <<< "$ids"
    echo
    if (( fail > 0 )); then
        err "$fail resource(s) failed verification"
        return 1
    fi
    ok "all requested resources verified"
}

cmd_list() {
    local filter_cat=""
    while (( $# )); do
        case "$1" in
            --category|-c) filter_cat="$2"; shift 2 ;;
            *) shift ;;
        esac
    done

    echo -e "${BOLD}helm-ng resources${RESET} ($MANIFEST)"
    echo
    printf "  ${BOLD}%-28s %-10s %-10s %s${RESET}\n" "ID" "ARCH" "CATEGORY" "DESCRIPTION"
    printf "  %-28s %-10s %-10s %s\n" "---" "----" "--------" "-----------"

    while IFS= read -r id; do
        local cat
        cat="$(resource_field "$id" '.category')"
        if [ -n "$filter_cat" ] && [ "$cat" != "$filter_cat" ]; then
            continue
        fi
        local desc arch
        desc="$(resource_field "$id" '.description')"
        arch="$(resource_field "$id" '.architecture')"
        printf "  %-28s %-10s %-10s %s\n" "$id" "$arch" "$cat" "$desc"
    done < <(resource_ids)
    echo
}

cmd_status() {
    local ids
    ids="$(resolve_ids "$@")"

    echo -e "${BOLD}helm-ng asset status${RESET}"
    echo
    printf "  ${BOLD}%-28s %-10s %-10s %-10s %s${RESET}\n" "ID" "ARCH" "CATEGORY" "STATUS" "DESCRIPTION"
    printf "  %-28s %-10s %-10s %-10s %s\n" "---" "----" "--------" "------" "-----------"

    while IFS= read -r id; do
        status_one "$id"
    done <<< "$ids"
    echo
}

cmd_clean() {
    local ids
    ids="$(resolve_ids "$@")"
    while IFS= read -r id; do
        clean_one "$id"
    done <<< "$ids"
}

# ── usage ────────────────────────────────────────────────────────────────────

usage() {
    cat <<'EOF'
helm-ng asset manager

Usage:
  scripts/manage-assets.sh <command> [options] [resource-id...]

Commands:
  download [--all | ID...]    Download and verify resources
  verify   [--all | ID...]    Verify checksums of local assets
  list     [--category CAT]   List all known resources
  status   [--all | ID...]    Show present/missing status
  clean    [--all | ID...]    Remove downloaded assets

Examples:
  scripts/manage-assets.sh download --all          # fetch everything
  scripts/manage-assets.sh download l4re-hello      # fetch one resource
  scripts/manage-assets.sh status                   # show what's present
  scripts/manage-assets.sh verify linux-rpi-kernel   # verify one resource
  scripts/manage-assets.sh list --category boot     # list boot resources
EOF
}

# ── main ─────────────────────────────────────────────────────────────────────

main() {
    check_deps

    if (( $# == 0 )); then
        usage
        exit 0
    fi

    local cmd="$1"; shift

    case "$cmd" in
        download)  cmd_download "$@" ;;
        verify)    cmd_verify "$@" ;;
        list)      cmd_list "$@" ;;
        status)    cmd_status "$@" ;;
        clean)     cmd_clean "$@" ;;
        help|-h|--help) usage ;;
        *)         die "unknown command: $cmd (try 'help')" ;;
    esac
}

main "$@"
