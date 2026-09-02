# Install the compiled package object plus the bundled C ABI library, so the
# installed package is self-contained: on Windows wickra_exchange.dll (matched
# by the *.dll glob); on Linux libwickra_exchange.so (matched by the SHLIB_EXT
# glob); on macOS libwickra_exchange.dylib, added explicitly because R package
# objects use the .so extension there too. The rpath baked by configure
# ($ORIGIN / @loader_path) resolves it from this libs directory.
files <- unique(c(Sys.glob(paste0("*", SHLIB_EXT)), Sys.glob("libwickra_exchange.dylib")))
dest <- file.path(R_PACKAGE_DIR, paste0("libs", R_ARCH))
dir.create(dest, recursive = TRUE, showWarnings = FALSE)
file.copy(files, dest, overwrite = TRUE)
if (file.exists("symbols.rds")) {
  file.copy("symbols.rds", dest, overwrite = TRUE)
}
