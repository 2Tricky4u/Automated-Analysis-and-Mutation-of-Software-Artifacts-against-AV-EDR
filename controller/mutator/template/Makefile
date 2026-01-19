CC = clang
CFLAGS = -target x86_64-pc-windows-msvc -fno-stack-protector
LDFLAGS = -fuse-ld=lld -Wl,/subsystem:console

# xwin paths (override if needed)
XWIN_DIR ?= $(HOME)/.xwin
INCLUDES = -isystem $(XWIN_DIR)/crt/include \
           -isystem $(XWIN_DIR)/sdk/include/ucrt \
           -isystem $(XWIN_DIR)/sdk/include/shared \
           -isystem $(XWIN_DIR)/sdk/include/um \
           -isystem $(XWIN_DIR)/sdk/include/winrt
LIBS = -L $(XWIN_DIR)/crt/lib/x86_64 \
       -L $(XWIN_DIR)/sdk/lib/x86_64

TARGET = loader.exe
SRC = loader.c

all: $(TARGET)

$(TARGET): $(SRC)
	$(CC) $(CFLAGS) $(INCLUDES) $(LIBS) $(LDFLAGS) -o $@ $<

clean:
	rm -f $(TARGET)
