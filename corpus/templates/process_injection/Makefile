# Makefile for process_injection template

CC := gcc
TARGET := process_injection.exe
SOURCE := process_injection.c

CFLAGS := -Wall -Wextra -O2
LDFLAGS := -mconsole

all: $(TARGET)

$(TARGET): $(SOURCE)
	@echo "[BUILD] Compiling $(TARGET)..."
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $<
	@echo "[DONE] $(TARGET) built successfully"
	@echo "[WARN] This uses classic malware injection patterns!"

clean:
	rm -f $(TARGET)

test: $(TARGET)
	@echo "[TEST] Running $(TARGET)..."
	@echo "[INFO] Will inject into notepad.exe"
	./$(TARGET)

.PHONY: all clean test
