# Copies the runtime DLLs and native launcher ported to Rust (built by `cargo build --release`) into
# runtime/build/bin, next to the C++ ones, where the native mode and anyxboxx.exe load them.
#
#   cmake -DRUST_RELEASE=<target/release> -DBIN=<runtime/build/bin> -DDLLS="src=dst,..." -P rust_dlls.cmake
#
# A missing Rust DLL stops the build with the command to run: no stale or partial runtime folder.
string(REPLACE "," ";" pairs "${DLLS}")
# file(COPY_FILE) does not create folders: on a clean build runtime/build/bin does not exist yet.
file(MAKE_DIRECTORY "${BIN}")
foreach(pair IN LISTS pairs)
  string(REPLACE "=" ";" names "${pair}")
  list(GET names 0 source)
  list(GET names 1 target)
  if(NOT EXISTS "${RUST_RELEASE}/${source}")
    message(FATAL_ERROR "${RUST_RELEASE}/${source} is missing: run `cargo build --release` at the "
                        "repository root before building runtime/ (docs/rust-full-status.md).")
  endif()
  file(COPY_FILE "${RUST_RELEASE}/${source}" "${BIN}/${target}" ONLY_IF_DIFFERENT)
endforeach()
