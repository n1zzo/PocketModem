#!/bin/bash
# Install shumate-1.0 pkg-config file for cross-compilation
# Run this on ht (Alpine) to enable libshumate development

# Check if we can write to system directories
if [ -w /usr/lib/pkgconfig ]; then
    DESTDIR="/usr/lib/pkgconfig"
elif [ -w /usr/local/lib/pkgconfig ]; then
    DESTDIR="/usr/local/lib/pkgconfig"
else
    echo "Need sudo to install pkg-config file"
    DESTDIR="/usr/lib/pkgconfig"
fi

# Create the pkg-config file
cat > /tmp/shumate-1.0.pc << 'EOF'
prefix=/usr
exec_prefix=${prefix}
libdir=/usr/lib
includedir=/usr/include

Name: shumate
Description: GTK4 map widget
Version: 1.6.3
Requires: gtk4 >= 4.10, glib-2.0, gobject-2.0, cairo, gdk-pixbuf-2.0
Libs: -L${libdir} -lshumate-1.0
Cflags: -I${includedir}/shumate-1.0
EOF

# Try to install with sudo
if sudo cp /tmp/shumate-1.0.pc "$DESTDIR/shumate-1.0.pc" 2>/dev/null; then
    echo "Installed to $DESTDIR/shumate-1.0.pc"
elif cp /tmp/shumate-1.0.pc "$DESTDIR/shumate-1.0.pc" 2>/dev/null; then
    echo "Installed to $DESTDIR/shumate-1.0.pc"
else
    # Create in local cargo directory
    CARGO_DIR="${CARGO_HOME:-$HOME/.cargo}"
    mkdir -p "$CARGO_DIR/pkgconfig"
    cp /tmp/shumate-1.0.pc "$CARGO_DIR/pkgconfig/shumate-1.0.pc"
    echo "Installed to $CARGO_DIR/pkgconfig/shumate-1.0.pc"
    echo "Set PKG_CONFIG_PATH=$CARGO_DIR/pkgconfig when building"
fi

# Verify
pkg-config --modversion shumate-1.0 && echo "Success!" || echo "Failed"