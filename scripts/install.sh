#!/bin/sh

# TrustTunnel Installation Script

set -e -u

# Function log is an echo wrapper that writes to stderr if the caller
# requested verbosity level greater than 0.  Otherwise, it does nothing.
log() {
  if [ "$verbose" -gt '0' ]
  then
    echo "$1" 1>&2
  fi
}

# Function error_exit is an echo wrapper that writes to stderr and stops the
# script execution with code 1.
error_exit() {
  echo "$1" 1>&2

  exit 1
}

# Function usage prints the note about how to use the script.
usage() {
  echo 'Usage: install.sh [-o output_dir] [-v] [-h] [-u] [-p archive_path] [-V version] [-a y|n]' 1>&2
}

# Function parse_opts parses the options list and validates it's combinations.
parse_opts() {
  while getopts 'vho:uV:p:a:' opt "$@"
  do
    case "$opt"
    in
    'v')
      verbose='1'
      ;;
    'h')
      usage
      exit 0
      ;;
    'o')
      output_dir="$OPTARG"
      ;;
    'u')
      uninstall='1'
      ;;
    'V')
      version="$OPTARG"
      ;;
    'p')
      archive_path="$OPTARG"
      ;;
    'a')
      case "$OPTARG" in
      [yY]|[yY][eE][sS])
        auto_answer="y"
        ;;
      [nN]|[nN][oO])
        auto_answer="n"
        ;;
      *)
        echo "Invalid value for -a. Use 'y' or 'n' (or 'yes'/'no')." 1>&2
        exit 1
        ;;
      esac
      ;;
    *)
      log "bad option $OPTARG"
      usage
    ;;
    esac
  done
}

# Function is_little_endian checks if the CPU is little-endian.
#
# See https://serverfault.com/a/163493/267530.
is_little_endian() {
  # The ASCII character "I" has the octal code of 111.  In the two-byte octal
  # display mode (-o), hexdump will print it either as "000111" on a little
  # endian system or as a "111000" on a big endian one.  Return the sixth
  # character to compare it against the number '1'.
  #
  # Do not use echo -n, because its behavior in the presence of the -n flag is
  # explicitly implementation-defined in POSIX.  Use hexdump instead of od,
  # because OpenWrt and its derivatives have the former but not the latter.
  is_little_endian_result="$(
    printf 'I'\
      | hexdump -o\
      | awk '{ print substr($2, 6, 1); exit; }'
  )"
  readonly is_little_endian_result

  [ "$is_little_endian_result" -eq '1' ]
}

# Function set_os sets the os if needed and validates the value.
set_os() {
  # Set if needed.
  if [ "$os" = '' ]
  then
    os="$( uname -s )"
    case "$os"
    in
    # Darwin packages are currently not available
    # ('Darwin')
    #   os='macos'
    #   ;;
    ('Linux')
      os='linux'
      ;;
    (*)
      error_exit "Unsupported operating system: '$os'"
      ;;
    esac
  fi

  # Validate.
  case "$os"
  in
  ('linux')
    # All right, go on.
    ;;
  (*)
    error_exit "Unsupported operating system: '$os'"
    ;;
  esac

  # Log.
  log "Operating system: $os"
}

# Function set_cpu sets the cpu if needed and validates the value.
set_cpu() {
  # Set if needed.
  if [ "$cpu" = '' ]
  then
    cpu="$( uname -m )"
    case "$cpu"
    in
    ('x86_64'|'x86-64'|'x64'|'amd64')
      cpu='x86_64'
      ;;
    ('armv7l' | 'armv8l')
      cpu='armv7'
      ;;
    ('aarch64'|'arm64')
      cpu='aarch64'
      ;;
    ('mips')
      if is_little_endian
      then
        cpu="mipsel"
      fi
      ;;
    (*)
      error_exit "unsupported cpu type: $cpu"
      ;;
    esac
  fi

  # Validate.
  case "$cpu"
  in
  ('x86_64'|'aarch64')
    # All right, go on.
    ;;
  (*)
    error_exit "Unsupported cpu type: $cpu"
    ;;
  esac

  # Log.
  log "CPU type: $cpu"
}

# Function for queries with support for auto-answer
ask_user() {
  local prompt="$1"
  printf "%s" "$prompt"
  if [ -n "$auto_answer" ]; then
    response="$auto_answer"
    echo "$response"
  else
    read -r response < /dev/tty
  fi
}

set_is_root() {
  if [ $USER = "root" ]; then
    is_root='1'
  fi
  if [ "$(id -u)" = "0" ]; then
    is_root='1'
  fi
}

# Function is_dir_owned_by_current_user checks if the output directory is owned by the current user
is_dir_owned_by_current_user() {
  dir="$1"

  if [ "$os" = "linux" ]; then
      # Linux
      if ! owner_name=$(stat -c '%U' "$dir"); then
          echo "Cannot stat '$dir'"
          return 0
      fi
  else
      echo "Unsupported OS: $os"
      return 1
  fi

  if [ "$owner_name" = "$USER" ]; then
      return 0
  else
      return 1
  fi
}

# Function create_dir creates the output directory if it does not exist.
create_dir() {
  mkdir -p "$output_dir" 2>/dev/null
  if [ $? -eq 1 ];
  then
    if [ "$is_root" -eq 0 ]; then
      echo "Requesting sudo to create directory '$output_dir'"
      if sudo mkdir -p "$output_dir"; then
        sudo chown -R "${SUDO_USER:-$USER}" "$output_dir"
        log "'$output_dir' has been created and ownership has been set to '${SUDO_USER:-$USER}'"
      else
        error_exit "Failed to create '$output_dir' with sudo"
      fi
    else
      error_exit "Failed to create '$output_dir'"
    fi
  else
    log "'$output_dir' has been created"
  fi
}

# Check if the directory is owned by the current user and change the ownership if needed
check_owner() {
  if [ "${is_root}" -eq 1 ]; then
    return 0
  fi
  if is_dir_owned_by_current_user "$output_dir";
  then
    log "'$output_dir' exists and is owned by '$USER'"
  else
    log "'$output_dir' exists but is not owned by '$USER'"
    ask_user "Would you like to change the ownership of $output_dir to $USER? [y/N] "
    case "$response" in
    [yY]|[yY][eE][sS])
      target_user="${SUDO_USER:-$USER}"
      if sudo chown -R "$target_user" "$output_dir"; then
        log "Ownership of $output_dir has been changed to '${target_user}'"
      else
        error_exit "Failed to change ownership of '$output_dir'"
      fi
      ;;
    *)
      error_exit "Installation cannot proceed without changing ownership of '$output_dir'"
      ;;
    esac
  fi
}

# Function check_out_dir requires the output directory to be set and exist.
check_out_dir() {
  if [ "$output_dir" = '' ]
  then
    # If output_dir is not set, we will install to /opt
    output_dir='/opt'
  fi

  # If output_dir is '.'or '/opt', create inside it `trusttunnel` directory
  if [ "$output_dir" = '.' ] || [ "$output_dir" = '/opt' ]
  then
    output_dir="${output_dir}/trusttunnel"
  fi

  if [ "$uninstall" -eq '1' ]
  then
    echo "TrustTunnel will be uninstalled from '$output_dir'"
    return 0
  else
    echo "TrustTunnel will be installed to '$output_dir'"
  fi

  set +e
  # Check if directory exists
  if [ ! -d "$output_dir" ];
  then
    log "'$output_dir' directory does not exist, attempting to create it..."
    create_dir
  else
    log "'$output_dir' directory exists"
    check_owner
  fi
  set -e
}

# TODO: enable this once binaries are signed with gpg
# Function verify_hint prints a hint about how to verify the installation.
# verify_hint() {
#   # Check if `.sig` file exists
#   if [ -f "${output_dir}/${exe_name}.sig" ]
#   then
#     echo
#     echo "To verify the installation, run the following command to import the public key and verify the signature:"
#     echo "    gpg --keyserver 'keys.openpgp.org' --recv-key '28645AC9776EC4C00BCE2AFC0FE641E7235E2EC6'"
#     echo "    gpg --verify ${output_dir}/${exe_name}.sig ${output_dir}/${exe_name}"
#   fi
# }

# Function remove_downloaded_package deletes the archive only if it was downloaded and not transferred.
remove_downloaded_package() {
  if [ -z "$archive_path" ]; then
    $remove_command "$pkg_name"
  fi
}

# Function unpack unpacks the passed archive depending on it's extension.
unpack() {
  log "Unpacking package from '$pkg_name' into '$output_dir'"
  if ! mkdir -p "$output_dir"
  then
    error_exit "Cannot create directory '$output_dir'"
  fi

  if ! tar -C "$output_dir" -f "$pkg_name" -x -z
  then
    remove_downloaded_package
    error_exit "Cannot unpack '$pkg_name'"
  fi

  remove_downloaded_package
  log "Package has been unpacked successfully"

  base_name=$(echo "$pkg_name" | sed -E 's!.*/!!')
  dir_name=$(echo "${base_name}" | sed -E -e 's/(.*)(\.tar\.gz)/\1/')
  if [ -z "${dir_name}" ]; then
    error_exit "Can not determine directory name inside archive"
  fi
  if type mv > /dev/null 2>&1; then
    mv -f "${output_dir}/${dir_name}/"* "${output_dir}"
  elif type ln > /dev/null 2>&1; then
    ln -f "${output_dir}/${dir_name}/"* "${output_dir}"
    rm -f "${output_dir}/${dir_name}/"*
  else
    error_exit "You need mv or ln for this script to work. Make sure that \"coreutils-mv\" is installed."
  fi
  if type rmdir > /dev/null 2>&1; then
    rmdir "${output_dir}/${dir_name}"
  else
    if [ "$(echo "${output_dir}/${dir_name}/"*)" != "${output_dir}/${dir_name}/*" ]; then
      error_exit "Directory not empty"
    fi
    rm -rf "${output_dir:?}/${dir_name:?}"
  fi
}

# Function unpack unpacks the passed archive depending on it's extension.
check_package() {
  if [ "$uninstall" -eq '1' ]; then
    return 0
  fi

  log "Checking downloaded package '$pkg_name'"
  case "$pkg_ext"
  in
  ('tar.gz')
    if ! tar -f "$pkg_name" -z -t > /dev/null
    then
      remove_downloaded_package
      error_exit "Error checking '$pkg_name'"
    fi
    ;;
  (*)
    error_exit "Unexpected package extension: '$pkg_ext'"
    ;;
  esac
}

# Function parse_version parses the version from the passed script and it's arguments.
parse_version() {
  if [ "$uninstall" -eq '1' ]; then
    return 0
  fi
  
  if [ -n "$version" ]; then
    return 0
  fi
  # Extract the base name of the script
  script_name="${0##*/}"

  version=$(echo "${script_name}" | sed -nE 's/.*([0-9]+\.[0-9]+\.[0-9]+).*/\1/p')
  if [ -z "$version" ]; then
    echo "Version required"
    return 1
  fi
  echo "Version number extracted from script name: $version"
}

# Add version to the package name if it's not empty.
apply_version() {
  if [ -z "$version" ]; then
    return 0
  fi

  pkg_name=$(echo "${pkg_name}" | sed -E "s/trusttunnel/trusttunnel-v${version}/")
}

# Main function.
configure() {
  if [ "$uninstall" -eq '1' ]
  then
    echo 'Uninstalling TrustTunnel...'
  else
    echo 'Installing TrustTunnel...'
  fi

  set_is_root
  set_os
  set_cpu
  check_out_dir

  pkg_ext='tar.gz'
  if [ -n "$archive_path" ]; then
    pkg_name="$archive_path"
    need_download='0'
    log "Using user-provided archive: $archive_path"
    return
  fi

  parse_version

  pkg_name="trusttunnel-${os}-${cpu}.${pkg_ext}"
  apply_version
  url="https://github.com/TrustTunnel/TrustTunnel/releases/download/v${version}/${pkg_name}"

  readonly output_dir url pkg_name

  if [ "$uninstall" -eq '0' ]
  then
    log "Package name: '$pkg_name'"
    log "TrustTunnel will be installed in '$output_dir'"
  fi
}

# Function handle_uninstall removes the existing package from the output directory.
handle_uninstall() {
  # Check if the package is present in the output directory.
  if [ ! -f "${output_dir}/trusttunnel_endpoint" ]
  then
    error_exit "TrustTunnel is not installed in '${output_dir}'. Please specify the correct installation directory"
  fi

  # Check if vpn is running
  if pgrep -x "trusttunnel_endpoint" > /dev/null
  then
    error_exit "TrustTunnel is currently running, please stop it before uninstalling"
  fi

  remove_existing

  # Check if the directory is empty
  if [ -z "$(ls -A "${output_dir}")" ]
  then
    set +e
    log "Remove empty directory: '${output_dir}'"
    rmdir "${output_dir}" 2>/dev/null
    if [ $? -eq 1 ];
    then
      if [ "$is_root" -eq 0 ]; then
        echo "Requesting sudo to remove '${output_dir}'"
        if sudo rmdir "${output_dir}"; then
          log "Empty directory '${output_dir}' has been removed"
        else
          error_exit "Failed to remove empty directory '${output_dir}' with sudo"
        fi
      else
        error_exit "Failed to remove empty directory '${output_dir}'"
      fi
    else
      log "Empty directory '${output_dir}' has been removed"
    fi
    set -e
  fi

  # Check systemd service
  if [ -L "${systemd_service_path}" ]
  then
    log "The systemd service for TrustTunnel is found: ${systemd_service_path}. If you want to remove it, do it manually"
  fi
}

# Function remove_existing removes the existing package from the output directory before installing a new one.
remove_existing() {
  log 'Removing existing package...'
  # Remove executable file
  rm -f "${output_dir}/trusttunnel_endpoint"
  log "'trusttunnel_endpoint' has been removed from '${output_dir}'"
  rm -f "${output_dir}/setup_wizard"
  log "'setup_wizard' has been removed from '${output_dir}'"
  rm -f "${output_dir}/trusttunnel.service.template"
  log "'trusttunnel.service.template' has been removed from '${output_dir}'"
  rm -f "${output_dir}/LICENSE"
  log "'LICENSE' has been removed from '${output_dir}'"
}

# Function checks if the package is already present in the output directory.
handle_existing() {
  if [ "$uninstall" -eq '1' ]
  then
    handle_uninstall
    exit 0
  fi

  if [ ! -f "${output_dir}/trusttunnel_endpoint" ]
  then
    return
  fi

  log "Package 'trusttunnel' is already present in '${output_dir}'"
  ask_user "Ensure 'TrustTunnel' is stopped before proceeding, continue? [y,N]"

  remove_existing
}

# Function download downloads the package from the url.
download() {
  if [ "$uninstall" -eq '1' ]; then
    return 0
  fi

  if [ "$need_download" -eq '0' ]; then
    return
  fi

  log "Downloading TrustTunnel package: $url"

  # Temporary file name for testing file creation
  tmp_file="tmp_file_check_$$"

  # Check if we can write to the current directory by creating a temporary file
  if ! touch "$tmp_file" > /dev/null 2>&1; then
    if [ "$is_root" -eq 0 ]; then
      ask_user "Cannot create file in the current directory. Try downloading as root? [y/N] "
      case "$response" in
      [yY]|[yY][eE][sS])
        remove_command="sudo rm -f"
        if ! sudo curl -fsSL "$url" -o "$pkg_name"; then
          error_exit "Failed to download $pkg_name: $?"
        fi
        ;;
      *)
        error_exit "Cannot proceed without file creation rights."
        ;;
      esac
    else
      log "Cannot create file in the current directory."
      error_exit "Cannot proceed without file creation rights."
    fi
  else
    # Cleanup the temporary file
    rm "$tmp_file"
    if ! curl -fsSL "$url" -o "$pkg_name"; then
      error_exit "Failed to download $pkg_name: $?"
    fi
  fi

  log "TrustTunnel package has been downloaded successfully"
}

report_success() {
  echo
  echo "============================================================"
  echo "  TrustTunnel Endpoint was installed to '${output_dir}'"
  echo "============================================================"
  echo
  echo "--- Continue setup ---"
  echo
  echo "Run the Setup Wizard to continue the endpoint setup:"
  echo
  echo "    cd ${output_dir}"
  echo "    sudo ./setup_wizard"
  echo
  echo "--- Configure systemd service ---"
  echo
  echo "The '${output_dir}/trusttunnel.service.template' template"
  echo "could be used to run a configured endpoint as a systemd service."
  echo
  echo "Once the endpoint setup is finished, run the following commands"
  echo "to configure a systemd service:"
  echo
  echo "    cd ${output_dir}/"
  echo "    sudo cp trusttunnel.service.template /etc/systemd/system/trusttunnel.service"
  echo "    sudo systemctl daemon-reload"
  echo "    sudo systemctl enable --now trusttunnel"
  echo
}

# Entrypoint

systemd_service_path='/etc/systemd/system/trusttunnel.service'
output_dir=''
verbose='1'
cpu=''
os=''
version='0.9.115'
uninstall='0'
remove_command="rm -f"
is_root='0'
archive_path=''
auto_answer=''
need_download='1'

parse_opts "$@"

configure
download

check_package

handle_existing

unpack

report_success
