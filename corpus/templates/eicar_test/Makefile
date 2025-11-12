# Makefile for eicar_test template

CC := gcc
TARGET := eicar_test.exe
SOURCE := eicar_test.c

CFLAGS := -Wall -Wextra -O2
LDFLAGS := -mconsole

all: $(TARGET)

$(TARGET): $(SOURCE)
	@echo "[BUILD] Compiling $(TARGET)..."
	$(CC) $(CFLAGS) $(LDFLAGS) -o $@ $<
	@echo "[DONE] $(TARGET) built successfully"
	@echo "[WARN] Contains EICAR signature - may be quarantined!"

clean:
	rm -f $(TARGET)

test: $(TARGET)
	@echo "[TEST] Running $(TARGET)..."
	@echo "[WARN] Your AV should detect this!"
	./$(TARGET)

.PHONY: all clean test
