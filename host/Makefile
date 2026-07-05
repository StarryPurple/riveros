CC      ?= gcc
CFLAGS  ?= -O2 -std=c11 -pthread
TARGET  = host_bench

all: $(TARGET)

$(TARGET): host_bench.c ring_adapter.o
	$(CC) $(CFLAGS) $^ -o $@

ring_adapter.o: ring_adapter.c ring_adapter.h
	$(CC) $(CFLAGS) -c $< -o $@

clean:
	rm -f *.o $(TARGET)

.PHONY: all clean
