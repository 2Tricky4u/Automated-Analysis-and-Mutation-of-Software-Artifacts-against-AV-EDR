# Makefile for loader_v1 template
#
# Targets:
#   make              - Build loader.exe (default)
#   make clean        - Remove build artifacts
#   make test         - Build and run (requires RedEDR/EDR to be running for full test)
#
# Requirements:
#   - MinGW-w64 (gcc, windres) in PATH
#   - Windows target (cross-compile on Linux: x86_64-w64-mingw32-gcc)

# Compiler and tools
CC := gcc
WINDRES := windres
RM := rm -f

# Output
TARGET := loader.exe
OBJS := loader.o loader_rc.o

# Compiler flags
CFLAGS := -Wall -Wextra -O2
LDFLAGS := -mconsole

# Default target
all: $(TARGET)

# Build executable
$(TARGET): $(OBJS)
	@echo "[LINK] Building $(TARGET)..."
	$(CC) $(LDFLAGS) -o $@ $^
	@echo "[DONE] $(TARGET) built successfully"

# Compile C source
loader.o: loader.c
	@echo "[CC] Compiling loader.c..."
	$(CC) $(CFLAGS) -c -o $@ $<

# Compile resource file
loader_rc.o: loader.rc
	@echo "[RC] Compiling loader.rc..."
	$(WINDRES) -i $< -o $@

# Clean build artifacts
clean:
	@echo "[CLEAN] Removing build artifacts..."
	$(RM) $(OBJS) $(TARGET)
	@echo "[CLEAN] Done"

# Test: build and run
test: $(TARGET)
	@echo "[TEST] Running $(TARGET)..."
	@echo "=========================================="
	./$(TARGET)
	@echo "=========================================="
	@echo "[TEST] Completed (check output above)"

# Show build info
info:
	@echo "Compiler:   $(shell $(CC) --version | head -1)"
	@echo "Windres:    $(shell $(WINDRES) --version | head -1)"
	@echo "Target:     $(TARGET)"
	@echo "Objects:    $(OBJS)"

.PHONY: all clean test info
