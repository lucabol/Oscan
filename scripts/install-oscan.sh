#!/usr/bin/env sh
set -eu

SOURCE_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
INSTALL_ROOT="$HOME/.local/share/oscan"
BIN_DIR="$HOME/.local/bin"
PROFILE=""
VERSION=""
EXPECTED_COMPILER_DIGEST=""
SET_DEFAULT=0
UNINSTALL=0
CREATE_LINK=1

usage() {
    echo "usage: $0 [--source-dir <path>] [--install-root <path>] [--bin-dir <path>] [--profile <name>] [--set-default] [--uninstall] [--no-bin-link]" >&2
}

fail() {
    echo "$1" >&2
    exit 1
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --source-dir)
            SOURCE_DIR="$2"
            shift 2
            ;;
        --install-root|--install-dir)
            INSTALL_ROOT="$2"
            shift 2
            ;;
        --bin-dir)
            BIN_DIR="$2"
            shift 2
            ;;
        --profile)
            PROFILE="$2"
            shift 2
            ;;
        --set-default)
            SET_DEFAULT=1
            shift
            ;;
        --uninstall)
            UNINSTALL=1
            shift
            ;;
        --no-bin-link)
            CREATE_LINK=0
            shift
            ;;
        *)
            usage
            exit 1
            ;;
    esac
done

case "$INSTALL_ROOT" in
    ""|"/"|"$HOME"|".")
        fail "refusing to use unsafe Oscan install root '$INSTALL_ROOT'"
        ;;
esac

mkdir -p "$INSTALL_ROOT"
INSTALL_ROOT="$(CDPATH= cd -- "$INSTALL_ROOT" && pwd -P)"
INSTALL_LOCK="$INSTALL_ROOT/.install.lock"
LOCK_HELD=0
BIN_LOCK=""
BIN_LOCK_HELD=0

release_install_lock() {
    if [ "$BIN_LOCK_HELD" -eq 1 ]; then
        rm -rf "$BIN_LOCK"
        BIN_LOCK_HELD=0
    fi
    if [ "$LOCK_HELD" -eq 1 ]; then
        rm -rf "$INSTALL_LOCK"
        LOCK_HELD=0
    fi
}

cleanup_install_lock() {
    _status="$?"
    trap - EXIT HUP INT TERM
    release_install_lock
    exit "$_status"
}

WAITED=0
while ! mkdir "$INSTALL_LOCK" 2>/dev/null; do
    if [ -f "$INSTALL_LOCK/pid" ]; then
        LOCK_OWNER="$(sed -n '1p' "$INSTALL_LOCK/pid")"
        case "$LOCK_OWNER" in
            ""|*[!0-9]*)
                rm -rf "$INSTALL_LOCK"
                continue
                ;;
            *)
                if ! kill -0 "$LOCK_OWNER" 2>/dev/null; then
                    rm -rf "$INSTALL_LOCK"
                    continue
                fi
                ;;
        esac
    fi
    [ "$WAITED" -lt 30 ] ||
        fail "timed out waiting for another Oscan install or uninstall to finish at '$INSTALL_ROOT'"
    WAITED=$((WAITED + 1))
    sleep 1
done
printf '%s\n' "$$" > "$INSTALL_LOCK/pid"
LOCK_HELD=1
trap cleanup_install_lock EXIT HUP INT TERM

if [ "$CREATE_LINK" -eq 1 ] || [ "$UNINSTALL" -eq 1 ]; then
    mkdir -p "$BIN_DIR"
    BIN_DIR="$(CDPATH= cd -- "$BIN_DIR" && pwd -P)"
    BIN_LOCK="$BIN_DIR/.oscan-selectors.lock"
    WAITED=0
    while ! mkdir "$BIN_LOCK" 2>/dev/null; do
        if [ -f "$BIN_LOCK/pid" ]; then
            LOCK_OWNER="$(sed -n '1p' "$BIN_LOCK/pid")"
            case "$LOCK_OWNER" in
                ""|*[!0-9]*)
                    rm -rf "$BIN_LOCK"
                    continue
                    ;;
                *)
                    if ! kill -0 "$LOCK_OWNER" 2>/dev/null; then
                        rm -rf "$BIN_LOCK"
                        continue
                    fi
                    ;;
            esac
        fi
        [ "$WAITED" -lt 30 ] ||
            fail "timed out waiting for another Oscan selector update to finish at '$BIN_DIR'"
        WAITED=$((WAITED + 1))
        sleep 1
    done
    printf '%s\n' "$$" > "$BIN_LOCK/pid"
    BIN_LOCK_HELD=1
fi

metadata_top_string() {
    sed -n "s/^  \"$1\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" \
        "$SOURCE_DIR/oscan-package.json" | sed -n '1p'
}

metadata_component_digest() {
    sed -n "s/^    \"$1\"[[:space:]]*:[[:space:]]*\"\([0-9a-fA-F]*\)\".*/\1/p" \
        "$SOURCE_DIR/oscan-package.json" | sed -n '1p'
}

METADATA="$SOURCE_DIR/oscan-package.json"
if [ -f "$METADATA" ]; then
    SCHEMA="$(sed -n 's/^  "schema_version"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p' "$METADATA" | sed -n '1p')"
    [ "$SCHEMA" = "2" ] || fail "unsupported oscan-package.json schema '$SCHEMA'; expected 2"
    PACKAGE_PROFILE="$(metadata_top_string profile)"
    PACKAGE_ID="$(metadata_top_string package_id)"
    VERSION="$(metadata_top_string version)"
    PACKAGE_TARGET="$(metadata_top_string target)"
    DEFAULT_BACKEND="$(metadata_top_string default_backend)"
    EXPECTED_COMPILER_DIGEST="$(metadata_component_digest oscan)"
    case "$PACKAGE_PROFILE" in
        full|llvm|cranelift|c) ;;
        *) fail "package metadata contains unknown profile '$PACKAGE_PROFILE'" ;;
    esac
    if [ -n "$PROFILE" ] && [ "$PROFILE" != "$PACKAGE_PROFILE" ]; then
        fail "requested profile '$PROFILE' does not match package profile '$PACKAGE_PROFILE'"
    fi
    PROFILE="$PACKAGE_PROFILE"
    [ "$PACKAGE_ID" = "oscan-$PROFILE" ] ||
        fail "package metadata package_id '$PACKAGE_ID' does not match profile '$PROFILE'"
    [ -n "$VERSION" ] || fail "package metadata does not declare a version"
    grep -Eq '^  "is_distribution"[[:space:]]*:[[:space:]]*true' "$METADATA" ||
        fail "package metadata must identify a packaged distribution"
    EXPECTED_DEFAULT="$PROFILE"
    [ "$PROFILE" != "full" ] || EXPECTED_DEFAULT="llvm"
    [ "$DEFAULT_BACKEND" = "$EXPECTED_DEFAULT" ] ||
        fail "package metadata default backend '$DEFAULT_BACKEND' does not match profile '$PROFILE' (expected '$EXPECTED_DEFAULT')"
    case "$(uname -s)" in
        Linux) EXPECTED_TARGET="linux-x86_64" ;;
        Darwin) EXPECTED_TARGET="macos-x86_64" ;;
        *) fail "unsupported installation host '$(uname -s)'" ;;
    esac
    case "$(uname -m)" in
        x86_64|amd64) ;;
        *) fail "package target '$EXPECTED_TARGET' requires an x86_64 host, not '$(uname -m)'" ;;
    esac
    [ "$PACKAGE_TARGET" = "$EXPECTED_TARGET" ] ||
        fail "package target '$PACKAGE_TARGET' cannot be installed on this host (expected '$EXPECTED_TARGET')"
    case "$EXPECTED_COMPILER_DIGEST" in
        *[!0-9a-fA-F]*|"") fail "package metadata does not declare a valid SHA-256 digest for oscan" ;;
    esac
    [ "${#EXPECTED_COMPILER_DIGEST}" -eq 64 ] ||
        fail "package metadata does not declare a valid SHA-256 digest for oscan"
elif [ "$UNINSTALL" -eq 1 ]; then
    case "$PROFILE" in
        full|llvm|cranelift|c|dev) ;;
        *) fail "--uninstall requires --profile full|llvm|cranelift|c|dev when package metadata is unavailable" ;;
    esac
elif [ "$PROFILE" = "dev" ]; then
    VERSION="current"
else
    fail "source bundle must contain schema-2 oscan-package.json; development installs must pass --profile dev"
fi

if [ "$UNINSTALL" -eq 0 ]; then
    case "$VERSION" in
        ""|"."|".."|*[!A-Za-z0-9._+-]*)
            fail "package metadata contains unsafe version '$VERSION'"
            ;;
    esac
    [ -f "$SOURCE_DIR/oscan" ] || fail "source bundle must contain an oscan binary"
    if [ -n "$EXPECTED_COMPILER_DIGEST" ]; then
        if command -v sha256sum >/dev/null 2>&1; then
            ACTUAL_COMPILER_DIGEST="$(sha256sum "$SOURCE_DIR/oscan")"
            ACTUAL_COMPILER_DIGEST="${ACTUAL_COMPILER_DIGEST%% *}"
        elif command -v shasum >/dev/null 2>&1; then
            ACTUAL_COMPILER_DIGEST="$(shasum -a 256 "$SOURCE_DIR/oscan")"
            ACTUAL_COMPILER_DIGEST="${ACTUAL_COMPILER_DIGEST%% *}"
        else
            fail "installing a release bundle requires sha256sum or shasum to verify oscan"
        fi
        [ "$ACTUAL_COMPILER_DIGEST" = "$EXPECTED_COMPILER_DIGEST" ] ||
            fail "oscan digest mismatch: package metadata has $EXPECTED_COMPILER_DIGEST, actual is $ACTUAL_COMPILER_DIGEST"
    fi
fi

PROFILES_ROOT="$INSTALL_ROOT/profiles"
PROFILE_ROOT="$PROFILES_ROOT/$PROFILE"
QUALIFIED_LINK="$BIN_DIR/oscan-$PROFILE"
QUALIFIED_OWNER="$BIN_DIR/.oscan-$PROFILE.owner"
DEFAULT_LINK="$BIN_DIR/oscan"
DEFAULT_OWNER="$BIN_DIR/.oscan-default.owner"
DEFAULT_STATE="$INSTALL_ROOT/default-profile"

atomic_symlink() {
    _target="$1"
    _path="$2"
    _temporary="$(dirname -- "$_path")/.tmp-$(basename -- "$_path")-$$"
    rm -f "$_temporary"
    if ! ln -s "$_target" "$_temporary"; then
        rm -f "$_temporary"
        return 1
    fi
    if ! mv -f "$_temporary" "$_path"; then
        rm -f "$_temporary"
        return 1
    fi
}

atomic_text() {
    _text="$1"
    _path="$2"
    _temporary="$(dirname -- "$_path")/.tmp-$(basename -- "$_path")-$$"
    printf '%s\n' "$_text" > "$_temporary"
    mv -f "$_temporary" "$_path"
}

selected_profile() {
    if [ -f "$DEFAULT_STATE" ]; then
        sed -n '1p' "$DEFAULT_STATE"
    fi
}

qualified_ownership() {
    _profile="$1"
    _link="$BIN_DIR/oscan-$_profile"
    _owner="$BIN_DIR/.oscan-$_profile.owner"
    if [ -e "$_link" ] && [ ! -L "$_link" ]; then
        printf '%s\n' unmanaged
    elif [ -f "$_owner" ]; then
        if [ "$(sed -n '1p' "$_owner")" = "$INSTALL_ROOT" ]; then
            printf '%s\n' current
        else
            printf '%s\n' foreign
        fi
    elif [ -L "$_link" ]; then
        case "$(readlink "$_link")" in
            "$PROFILES_ROOT/$_profile/"*) printf '%s\n' current ;;
            *) printf '%s\n' foreign ;;
        esac
    elif [ -e "$_link" ]; then
        printf '%s\n' unmanaged
    else
        printf '%s\n' absent
    fi
}

default_ownership() {
    _selected="$1"
    if [ -e "$DEFAULT_LINK" ] && [ ! -L "$DEFAULT_LINK" ]; then
        printf '%s\n' unmanaged
    elif [ -f "$DEFAULT_OWNER" ]; then
        if [ "$(sed -n '1p' "$DEFAULT_OWNER")" = "$INSTALL_ROOT" ]; then
            printf '%s\n' current
        else
            printf '%s\n' foreign
        fi
    elif [ -L "$DEFAULT_LINK" ]; then
        if [ -n "$_selected" ] &&
            [ "$(readlink "$DEFAULT_LINK")" = "oscan-$_selected" ] &&
            [ "$(qualified_ownership "$_selected")" = current ]; then
            printf '%s\n' current
        else
            printf '%s\n' foreign
        fi
    elif [ -e "$DEFAULT_LINK" ]; then
        printf '%s\n' unmanaged
    else
        printf '%s\n' absent
    fi
}

SELECTED="$(selected_profile)"
case "$SELECTED" in
    ""|full|llvm|cranelift|c|dev) ;;
    *) fail "default-profile contains unknown profile '$SELECTED'" ;;
esac
QUALIFIED_OWNERSHIP="$(qualified_ownership "$PROFILE")"
DEFAULT_OWNERSHIP="$(default_ownership "$SELECTED")"

if [ "$UNINSTALL" -eq 1 ]; then
    UNINSTALL_BACKUP="$PROFILES_ROOT/.$PROFILE-uninstall-$$"
    PAYLOAD_STAGED_FOR_REMOVAL=0
    QUALIFIED_LINK_REMOVED=0
    QUALIFIED_OWNER_REMOVED=0
    DEFAULT_LINK_REMOVED=0
    DEFAULT_OWNER_REMOVED=0
    STATE_REMOVED=0
    UNINSTALL_COMMITTED=0

    QUALIFIED_EXISTED=0
    QUALIFIED_TARGET=""
    if [ -L "$QUALIFIED_LINK" ]; then
        QUALIFIED_EXISTED=1
        QUALIFIED_TARGET="$(readlink "$QUALIFIED_LINK")"
    fi
    QUALIFIED_OWNER_EXISTED=0
    QUALIFIED_OWNER_VALUE=""
    if [ -f "$QUALIFIED_OWNER" ]; then
        QUALIFIED_OWNER_EXISTED=1
        QUALIFIED_OWNER_VALUE="$(sed -n '1p' "$QUALIFIED_OWNER")"
    fi
    DEFAULT_EXISTED=0
    DEFAULT_TARGET=""
    if [ -L "$DEFAULT_LINK" ]; then
        DEFAULT_EXISTED=1
        DEFAULT_TARGET="$(readlink "$DEFAULT_LINK")"
    fi
    DEFAULT_OWNER_EXISTED=0
    DEFAULT_OWNER_VALUE=""
    if [ -f "$DEFAULT_OWNER" ]; then
        DEFAULT_OWNER_EXISTED=1
        DEFAULT_OWNER_VALUE="$(sed -n '1p' "$DEFAULT_OWNER")"
    fi
    STATE_EXISTED=0
    STATE_VALUE="$SELECTED"
    [ ! -f "$DEFAULT_STATE" ] || STATE_EXISTED=1

    rollback_uninstall() {
        _status="$?"
        trap - EXIT HUP INT TERM
        _rollback_failed=0
        if [ "$UNINSTALL_COMMITTED" -eq 0 ]; then
            if [ "$PAYLOAD_STAGED_FOR_REMOVAL" -eq 1 ] &&
                [ -e "$UNINSTALL_BACKUP" ]; then
                mv "$UNINSTALL_BACKUP" "$PROFILE_ROOT" || {
                    echo "warning: could not restore profile payload '$PROFILE_ROOT'" >&2
                    _rollback_failed=1
                }
            fi
            if [ "$QUALIFIED_LINK_REMOVED" -eq 1 ] &&
                [ "$QUALIFIED_EXISTED" -eq 1 ]; then
                atomic_symlink "$QUALIFIED_TARGET" "$QUALIFIED_LINK" || {
                    echo "warning: could not restore '$QUALIFIED_LINK'" >&2
                    _rollback_failed=1
                }
            fi
            if [ "$QUALIFIED_OWNER_REMOVED" -eq 1 ] &&
                [ "$QUALIFIED_OWNER_EXISTED" -eq 1 ]; then
                atomic_text "$QUALIFIED_OWNER_VALUE" "$QUALIFIED_OWNER" || {
                    echo "warning: could not restore '$QUALIFIED_OWNER'" >&2
                    _rollback_failed=1
                }
            fi
            if [ "$DEFAULT_LINK_REMOVED" -eq 1 ] &&
                [ "$DEFAULT_EXISTED" -eq 1 ]; then
                atomic_symlink "$DEFAULT_TARGET" "$DEFAULT_LINK" || {
                    echo "warning: could not restore '$DEFAULT_LINK'" >&2
                    _rollback_failed=1
                }
            fi
            if [ "$DEFAULT_OWNER_REMOVED" -eq 1 ] &&
                [ "$DEFAULT_OWNER_EXISTED" -eq 1 ]; then
                atomic_text "$DEFAULT_OWNER_VALUE" "$DEFAULT_OWNER" || {
                    echo "warning: could not restore '$DEFAULT_OWNER'" >&2
                    _rollback_failed=1
                }
            fi
            if [ "$STATE_REMOVED" -eq 1 ] &&
                [ "$STATE_EXISTED" -eq 1 ]; then
                atomic_text "$STATE_VALUE" "$DEFAULT_STATE" || {
                    echo "warning: could not restore '$DEFAULT_STATE'" >&2
                    _rollback_failed=1
                }
            fi
        fi
        [ "$_rollback_failed" -eq 0 ] ||
            echo "warning: uninstall rollback was incomplete" >&2
        release_install_lock
        [ "$_status" -ne 0 ] || _status=1
        exit "$_status"
    }
    trap rollback_uninstall EXIT HUP INT TERM

    if [ -e "$PROFILE_ROOT" ]; then
        rm -rf "$UNINSTALL_BACKUP"
        mv "$PROFILE_ROOT" "$UNINSTALL_BACKUP"
        PAYLOAD_STAGED_FOR_REMOVAL=1
    fi
    if [ "$QUALIFIED_OWNERSHIP" = current ]; then
        if [ -L "$QUALIFIED_LINK" ]; then
            rm -f "$QUALIFIED_LINK"
            QUALIFIED_LINK_REMOVED=1
        fi
        if [ -f "$QUALIFIED_OWNER" ]; then
            rm -f "$QUALIFIED_OWNER"
            QUALIFIED_OWNER_REMOVED=1
        fi
    fi
    if [ "$SELECTED" = "$PROFILE" ]; then
        if [ "$DEFAULT_OWNERSHIP" = current ]; then
            if [ -L "$DEFAULT_LINK" ]; then
                rm -f "$DEFAULT_LINK"
                DEFAULT_LINK_REMOVED=1
            fi
            if [ -f "$DEFAULT_OWNER" ]; then
                rm -f "$DEFAULT_OWNER"
                DEFAULT_OWNER_REMOVED=1
            fi
        fi
        if [ -f "$DEFAULT_STATE" ]; then
            rm -f "$DEFAULT_STATE"
            STATE_REMOVED=1
        fi
    fi
    UNINSTALL_COMMITTED=1
    if [ "$PAYLOAD_STAGED_FOR_REMOVAL" -eq 1 ] &&
        ! rm -rf "$UNINSTALL_BACKUP"; then
        echo "warning: the profile was deactivated, but its staged payload could not be deleted from '$UNINSTALL_BACKUP'" >&2
    fi
    if [ "$QUALIFIED_OWNERSHIP" = foreign ] ||
        [ "$QUALIFIED_OWNERSHIP" = unmanaged ]; then
        echo "warning: preserved '$QUALIFIED_LINK' because it is not owned by install root '$INSTALL_ROOT'" >&2
    fi
    if [ "$SELECTED" = "$PROFILE" ]; then
        echo "warning: removed the selected '$PROFILE' profile; the unqualified oscan command now has no default for this install root" >&2
    fi
    trap - EXIT HUP INT TERM
    release_install_lock
    echo "Uninstalled Oscan profile '$PROFILE'; other profiles were preserved."
    exit 0
fi

VERSION_DIR="$PROFILE_ROOT/$VERSION"
mkdir -p "$PROFILE_ROOT"
STAGED="$PROFILE_ROOT/.install-$VERSION-$$"
BACKUP="$PROFILE_ROOT/.backup-$VERSION-$$"
rm -rf "$STAGED" "$BACKUP"

QUALIFIED_EXISTED=0
QUALIFIED_TARGET=""
QUALIFIED_OWNER_EXISTED=0
QUALIFIED_OWNER_VALUE=""
if [ "$CREATE_LINK" -eq 1 ]; then
    case "$QUALIFIED_OWNERSHIP" in
        foreign|unmanaged)
            fail "refusing to replace command '$QUALIFIED_LINK' because it is not owned by install root '$INSTALL_ROOT'"
            ;;
    esac
    if [ -L "$QUALIFIED_LINK" ]; then
        QUALIFIED_EXISTED=1
        QUALIFIED_TARGET="$(readlink "$QUALIFIED_LINK")"
    fi
    if [ -f "$QUALIFIED_OWNER" ]; then
        QUALIFIED_OWNER_EXISTED=1
        QUALIFIED_OWNER_VALUE="$(sed -n '1p' "$QUALIFIED_OWNER")"
    fi
fi

if [ "$CREATE_LINK" -eq 1 ] &&
    [ -n "$SELECTED" ] &&
    [ "$DEFAULT_OWNERSHIP" != current ]; then
    fail "default-profile state at '$DEFAULT_STATE' does not own shared selector '$DEFAULT_LINK'"
fi
SET_DEFAULT_NOW=0
if [ "$CREATE_LINK" -eq 1 ] &&
    { [ "$SET_DEFAULT" -eq 1 ] || { [ -z "$SELECTED" ] && [ ! -e "$DEFAULT_LINK" ] && [ ! -L "$DEFAULT_LINK" ]; }; }; then
    SET_DEFAULT_NOW=1
fi
if [ "$SET_DEFAULT_NOW" -eq 1 ]; then
    case "$DEFAULT_OWNERSHIP" in
        foreign|unmanaged)
            fail "refusing to replace default command '$DEFAULT_LINK' because it is not owned by install root '$INSTALL_ROOT'"
            ;;
    esac
fi

DEFAULT_EXISTED=0
DEFAULT_TARGET=""
DEFAULT_OWNER_EXISTED=0
DEFAULT_OWNER_VALUE=""
STATE_EXISTED=0
STATE_VALUE="$SELECTED"
if [ "$SET_DEFAULT_NOW" -eq 1 ]; then
    if [ -L "$DEFAULT_LINK" ]; then
        DEFAULT_EXISTED=1
        DEFAULT_TARGET="$(readlink "$DEFAULT_LINK")"
    elif [ -e "$DEFAULT_LINK" ]; then
        fail "refusing to replace unmanaged command '$DEFAULT_LINK'"
    fi
    if [ -f "$DEFAULT_OWNER" ]; then
        DEFAULT_OWNER_EXISTED=1
        DEFAULT_OWNER_VALUE="$(sed -n '1p' "$DEFAULT_OWNER")"
    fi
    [ ! -f "$DEFAULT_STATE" ] || STATE_EXISTED=1
elif [ "$DEFAULT_OWNERSHIP" = current ] && [ -f "$DEFAULT_OWNER" ]; then
    DEFAULT_OWNER_EXISTED=1
    DEFAULT_OWNER_VALUE="$(sed -n '1p' "$DEFAULT_OWNER")"
fi

PAYLOAD_ACTIVATED=0
BACKUP_CREATED=0
QUALIFIED_CHANGED=0
DEFAULT_CHANGED=0
DEFAULT_OWNER_CHANGED=0
COMMITTED=0

rollback_install() {
    _status="$?"
    trap - EXIT HUP INT TERM
    if [ "$COMMITTED" -eq 0 ]; then
        if [ "$PAYLOAD_ACTIVATED" -eq 1 ]; then
            rm -rf "$VERSION_DIR" ||
                echo "warning: could not remove failed payload '$VERSION_DIR'" >&2
        fi
        if [ "$BACKUP_CREATED" -eq 1 ] && [ -e "$BACKUP" ]; then
            mv "$BACKUP" "$VERSION_DIR" ||
                echo "warning: could not restore previous payload from '$BACKUP'" >&2
        fi
        if [ "$QUALIFIED_CHANGED" -eq 1 ]; then
            rm -f "$QUALIFIED_LINK" "$QUALIFIED_OWNER"
            if [ "$QUALIFIED_EXISTED" -eq 1 ]; then
                atomic_symlink "$QUALIFIED_TARGET" "$QUALIFIED_LINK" ||
                    echo "warning: could not restore '$QUALIFIED_LINK'" >&2
            fi
            if [ "$QUALIFIED_OWNER_EXISTED" -eq 1 ]; then
                atomic_text "$QUALIFIED_OWNER_VALUE" "$QUALIFIED_OWNER" ||
                    echo "warning: could not restore '$QUALIFIED_OWNER'" >&2
            fi
        fi
        if [ "$DEFAULT_CHANGED" -eq 1 ]; then
            rm -f "$DEFAULT_LINK"
            if [ "$DEFAULT_EXISTED" -eq 1 ]; then
                atomic_symlink "$DEFAULT_TARGET" "$DEFAULT_LINK" ||
                    echo "warning: could not restore '$DEFAULT_LINK'" >&2
            fi
            if [ "$STATE_EXISTED" -eq 1 ]; then
                atomic_text "$STATE_VALUE" "$DEFAULT_STATE" ||
                    echo "warning: could not restore '$DEFAULT_STATE'" >&2
            else
                rm -f "$DEFAULT_STATE"
            fi
        fi
        if [ "$DEFAULT_OWNER_CHANGED" -eq 1 ]; then
            rm -f "$DEFAULT_OWNER"
            if [ "$DEFAULT_OWNER_EXISTED" -eq 1 ]; then
                atomic_text "$DEFAULT_OWNER_VALUE" "$DEFAULT_OWNER" ||
                    echo "warning: could not restore '$DEFAULT_OWNER'" >&2
            fi
        fi
    fi
    rm -rf "$STAGED"
    if [ "$COMMITTED" -eq 1 ]; then
        rm -rf "$BACKUP"
    fi
    if [ "$_status" -eq 0 ] && [ "$COMMITTED" -eq 0 ]; then
        _status=1
    fi
    release_install_lock
    exit "$_status"
}
trap rollback_install EXIT HUP INT TERM

mkdir -p "$STAGED"
cp -RP "$SOURCE_DIR"/. "$STAGED"/
[ -f "$STAGED/oscan" ] || fail "staged profile is missing oscan"
if [ -n "$EXPECTED_COMPILER_DIGEST" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
        STAGED_COMPILER_DIGEST="$(sha256sum "$STAGED/oscan")"
        STAGED_COMPILER_DIGEST="${STAGED_COMPILER_DIGEST%% *}"
    else
        STAGED_COMPILER_DIGEST="$(shasum -a 256 "$STAGED/oscan")"
        STAGED_COMPILER_DIGEST="${STAGED_COMPILER_DIGEST%% *}"
    fi
    [ "$STAGED_COMPILER_DIGEST" = "$EXPECTED_COMPILER_DIGEST" ] ||
        fail "staged oscan digest mismatch: package metadata has $EXPECTED_COMPILER_DIGEST, actual is $STAGED_COMPILER_DIGEST"
fi
if [ -f "$METADATA" ]; then
    [ -f "$STAGED/oscan-package.json" ] || fail "staged profile is missing oscan-package.json"
fi
chmod +x "$STAGED/oscan"
[ ! -f "$STAGED/install.sh" ] || chmod +x "$STAGED/install.sh"

if [ -e "$VERSION_DIR" ]; then
    BACKUP_CREATED=1
    mv "$VERSION_DIR" "$BACKUP"
fi
PAYLOAD_ACTIVATED=1
mv "$STAGED" "$VERSION_DIR" || fail "could not activate the staged '$PROFILE' profile"

if [ "$CREATE_LINK" -eq 1 ]; then
    mkdir -p "$BIN_DIR"
    QUALIFIED_CHANGED=1
    atomic_symlink "$VERSION_DIR/oscan" "$QUALIFIED_LINK" ||
        fail "could not activate the oscan-$PROFILE command"
    atomic_text "$INSTALL_ROOT" "$QUALIFIED_OWNER"

    if [ "$SET_DEFAULT_NOW" -eq 1 ]; then
        DEFAULT_CHANGED=1
        atomic_symlink "oscan-$PROFILE" "$DEFAULT_LINK"
        atomic_text "$PROFILE" "$DEFAULT_STATE"
        SELECTED="$PROFILE"
    fi
    if [ -n "$SELECTED" ] &&
        { [ "$SET_DEFAULT_NOW" -eq 1 ] || [ "$DEFAULT_OWNERSHIP" = current ]; }; then
        DEFAULT_OWNER_CHANGED=1
        atomic_text "$INSTALL_ROOT" "$DEFAULT_OWNER"
    fi
fi

COMMITTED=1
# The qualified link now selects the new payload. Remove old versions only
# after activation, so an interrupted copy cannot destroy the working profile.
# With --no-bin-link, retain older versions because an existing managed link
# may still reference one and the caller explicitly opted out of retargeting it.
if [ "$CREATE_LINK" -eq 1 ]; then
    for OLD in "$PROFILE_ROOT"/*; do
        [ -e "$OLD" ] || continue
        [ "$OLD" = "$VERSION_DIR" ] || rm -rf "$OLD"
    done
fi
rm -rf "$PROFILE_ROOT"/.install-* "$PROFILE_ROOT"/.backup-*
trap - EXIT HUP INT TERM
release_install_lock

LEGACY_ROOT="$HOME/.local/oscan"
if [ -f "$LEGACY_ROOT/oscan" ] && [ "$LEGACY_ROOT" != "$INSTALL_ROOT" ]; then
    echo "warning: legacy flat install remains at $LEGACY_ROOT; remove it after confirming the new profile works" >&2
fi

echo "Installed Oscan profile '$PROFILE' to $VERSION_DIR"
if [ "$CREATE_LINK" -eq 1 ]; then
    echo "Qualified command: $QUALIFIED_LINK"
    if [ "$SELECTED" = "$PROFILE" ]; then
        echo "Default command: $DEFAULT_LINK -> oscan-$PROFILE"
    else
        echo "Default remains '$SELECTED'. Re-run with --set-default to select '$PROFILE'."
    fi
else
    echo "Run $VERSION_DIR/oscan directly or add a qualified link yourself."
fi
