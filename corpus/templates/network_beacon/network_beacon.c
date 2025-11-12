/**
 * Network Beacon Template
 *
 * Suspicious Pattern: Periodic HTTP callbacks (C2 beacon simulation)
 * EDR Trigger: Repeated outbound connections to external IP every 60s
 * Benign Effect: Connects to example.com (legitimate site), sends harmless data
 *
 * Detection likelihood: HIGH (C2 communication pattern)
 */

// Must include winsock2 BEFORE windows.h to avoid conflicts
#define WIN32_LEAN_AND_MEAN
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <stdio.h>

#pragma comment(lib, "ws2_32.lib")

#define BEACON_INTERVAL_SEC 60
#define C2_HOST "example.com"  // Benign target
#define C2_PORT 80
#define MAX_BEACONS 3  // Limit for testing

// Simulate C2 beacon payload (benign HTTP request)
int send_beacon(int beacon_num) {
    printf("[BEACON %d] Initiating connection to %s:%d...\n", beacon_num, C2_HOST, C2_PORT);

    // === SUSPICIOUS: Network socket creation ===
    SOCKET sock = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (sock == INVALID_SOCKET) {
        fprintf(stderr, "[ERROR] socket() failed: %d\n", WSAGetLastError());
        return -1;
    }

    // Resolve hostname
    struct addrinfo hints = {0}, *result = NULL;
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_protocol = IPPROTO_TCP;

    char port_str[6];
    snprintf(port_str, sizeof(port_str), "%d", C2_PORT);

    if (getaddrinfo(C2_HOST, port_str, &hints, &result) != 0) {
        fprintf(stderr, "[ERROR] getaddrinfo() failed: %d\n", WSAGetLastError());
        closesocket(sock);
        return -1;
    }

    // === SUSPICIOUS: Outbound connection ===
    printf("[SUSPICIOUS] Connecting to remote host...\n");
    if (connect(sock, result->ai_addr, (int)result->ai_addrlen) == SOCKET_ERROR) {
        fprintf(stderr, "[ERROR] connect() failed: %d\n", WSAGetLastError());
        freeaddrinfo(result);
        closesocket(sock);
        return -1;
    }

    freeaddrinfo(result);
    printf("[SUSPICIOUS] Connection established!\n");

    // === SUSPICIOUS: Send data (mimics C2 check-in) ===
    char beacon_data[256];
    snprintf(beacon_data, sizeof(beacon_data),
        "GET / HTTP/1.1\r\n"
        "Host: %s\r\n"
        "User-Agent: BeaconTest/%d\r\n"
        "X-Session-ID: %08X\r\n"  // Looks like malware session ID
        "Connection: close\r\n"
        "\r\n",
        C2_HOST, beacon_num, GetCurrentProcessId()
    );

    printf("[SUSPICIOUS] Sending beacon data (%d bytes)...\n", (int)strlen(beacon_data));
    if (send(sock, beacon_data, (int)strlen(beacon_data), 0) == SOCKET_ERROR) {
        fprintf(stderr, "[ERROR] send() failed: %d\n", WSAGetLastError());
        closesocket(sock);
        return -1;
    }

    // === SUSPICIOUS: Receive response (C2 commands simulation) ===
    char response[1024];
    int bytes_received = recv(sock, response, sizeof(response) - 1, 0);
    if (bytes_received > 0) {
        response[bytes_received] = '\0';
        printf("[SUSPICIOUS] Received %d bytes from C2:\n", bytes_received);
        printf("--- Response Preview ---\n");
        // Only show first 200 chars
        if (bytes_received > 200) {
            response[200] = '\0';
        }
        printf("%s\n", response);
        printf("--- End Response ---\n");
    } else if (bytes_received == 0) {
        printf("[INFO] Connection closed by server\n");
    } else {
        fprintf(stderr, "[ERROR] recv() failed: %d\n", WSAGetLastError());
    }

    closesocket(sock);
    printf("[BEACON %d] Connection closed\n", beacon_num);

    return 0;
}

int main(void) {
    printf("[NETWORK_BEACON] C2 beacon pattern test\n");
    printf("[NETWORK_BEACON] PID: %lu\n", GetCurrentProcessId());
    printf("\n");
    printf("[WARNING] This mimics Command & Control (C2) behavior!\n");
    printf("[INFO] Target: %s (benign test site)\n", C2_HOST);
    printf("[INFO] Beacon interval: %d seconds\n", BEACON_INTERVAL_SEC);
    printf("[INFO] Will send %d beacons total\n", MAX_BEACONS);
    printf("\n");

    // Initialize Winsock
    WSADATA wsaData;
    if (WSAStartup(MAKEWORD(2, 2), &wsaData) != 0) {
        fprintf(stderr, "[ERROR] WSAStartup failed: %d\n", WSAGetLastError());
        return 1;
    }

    printf("[INFO] Winsock initialized\n");
    printf("\n");

    // === SUSPICIOUS: Beacon loop (C2 check-in pattern) ===
    for (int i = 1; i <= MAX_BEACONS; i++) {
        printf("========================================\n");
        printf("  Beacon %d/%d\n", i, MAX_BEACONS);
        printf("========================================\n");

        if (send_beacon(i) == 0) {
            printf("[SUCCESS] Beacon %d completed\n", i);
        } else {
            printf("[FAILED] Beacon %d failed\n", i);
        }

        if (i < MAX_BEACONS) {
            printf("\n[INFO] Waiting %d seconds until next beacon...\n", BEACON_INTERVAL_SEC);
            printf("[INFO] This periodic pattern looks like malware C2!\n");
            printf("\n");

            // Sleep in chunks to allow early termination
            for (int j = 0; j < BEACON_INTERVAL_SEC; j++) {
                Sleep(1000);
                if ((j + 1) % 10 == 0) {
                    printf("  ... %d seconds elapsed\n", j + 1);
                }
            }
        }
    }

    printf("\n========================================\n");
    printf("[INFO] All beacons completed\n");
    printf("[INFO] In real malware, this would continue indefinitely\n");
    printf("========================================\n");

    WSACleanup();

    printf("\n[NETWORK_BEACON] Test complete - press Enter to exit\n");
    getchar();

    return 0;
}
