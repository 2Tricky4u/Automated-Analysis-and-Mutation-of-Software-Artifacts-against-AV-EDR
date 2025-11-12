# Makefile for network_beacon template

CC := gcc
TARGET := network_beacon.exe
SOURCE := network_beacon.c

CFLAGS := -Wall -Wextra -O2
LDFLAGS := -mconsole -lws2_32

all: $(TARGET)

$(TARGET): $(SOURCE)
	@echo "[BUILD] Compiling $(TARGET)..."
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $<
	@echo "[DONE] $(TARGET) built successfully"
	@echo "[WARN] This uses C2 beacon patterns!"

clean:
	rm -f $(TARGET)

test: $(TARGET)
	@echo "[TEST] Running $(TARGET)..."
	@echo "[INFO] Will connect to example.com (benign)"
	./$(TARGET)

.PHONY: all clean test
