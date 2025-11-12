# Makefile for rwx_direct template

CC := gcc
TARGET := rwx_direct.exe
SOURCE := rwx_direct.c

CFLAGS := -Wall -Wextra -O2
LDFLAGS := -mconsole

all: $(TARGET)

$(TARGET): $(SOURCE)
	@echo "[BUILD] Compiling $(TARGET)..."
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $<
	@echo "[DONE] $(TARGET) built successfully"

clean:
	rm -f $(TARGET)

test: $(TARGET)
	@echo "[TEST] Running $(TARGET)..."
	./$(TARGET)

.PHONY: all clean test
