#!/usr/bin/env python3
"""
Mock VNC Server for Testing noVNC WebSocket Proxy

This script simulates a basic VNC server that implements the RFB protocol
handshake, allowing us to test the noVNC WebSocket proxy functionality
without requiring actual QEMU VMs (useful for WSL2 testing).

RFB Protocol Flow:
1. Server sends version string (RFB 003.008)
2. Client responds with version
3. Server sends security types (1 = None)
4. Client selects security type
5. Server sends security result (0 = OK)
6. Client sends ClientInit
7. Server sends ServerInit (framebuffer parameters)
8. Normal operation (client/server messages)
"""

import socket
import struct
import sys
import time
import threading

class MockVNCServer:
    """Simulates a VNC server for testing purposes"""

    RFB_VERSION = b"RFB 003.008\n"
    SECURITY_NONE = 1

    def __init__(self, host='127.0.0.1', port=5900):
        self.host = host
        self.port = port
        self.running = False
        self.clients = []

    def start(self):
        """Start the mock VNC server"""
        self.server_socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.server_socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)

        try:
            self.server_socket.bind((self.host, self.port))
            self.server_socket.listen(5)
            self.running = True

            print(f"Mock VNC server listening on {self.host}:{self.port}")
            print(f"RFB Version: {self.RFB_VERSION.decode().strip()}")
            print("Waiting for connections...")
            print()

            while self.running:
                try:
                    client_socket, client_address = self.server_socket.accept()
                    print(f"[{time.strftime('%H:%M:%S')}] Client connected: {client_address}")

                    # Handle client in a separate thread
                    client_thread = threading.Thread(
                        target=self.handle_client,
                        args=(client_socket, client_address)
                    )
                    client_thread.daemon = True
                    client_thread.start()
                    self.clients.append((client_socket, client_thread))

                except KeyboardInterrupt:
                    break
                except Exception as e:
                    print(f"Error accepting connection: {e}")

        finally:
            self.stop()

    def handle_client(self, client_socket, client_address):
        """Handle a single VNC client connection"""
        try:
            # Step 1: Send RFB version
            client_socket.sendall(self.RFB_VERSION)
            print(f"[{time.strftime('%H:%M:%S')}] -> Sent RFB version to {client_address}")

            # Step 2: Receive client version
            client_version = client_socket.recv(12)
            if len(client_version) == 12:
                print(f"[{time.strftime('%H:%M:%S')}] <- Received client version: {client_version.decode().strip()}")
            else:
                print(f"[{time.strftime('%H:%M:%S')}] <- Invalid client version length: {len(client_version)}")
                return

            # Step 3: Send security types (1 type: None)
            security_types = struct.pack('BB', 1, self.SECURITY_NONE)  # 1 type, type=None
            client_socket.sendall(security_types)
            print(f"[{time.strftime('%H:%M:%S')}] -> Sent security types: [None]")

            # Step 4: Receive client security selection
            security_selection = client_socket.recv(1)
            if len(security_selection) == 1:
                selected = struct.unpack('B', security_selection)[0]
                print(f"[{time.strftime('%H:%M:%S')}] <- Client selected security type: {selected}")
            else:
                print(f"[{time.strftime('%H:%M:%S')}] <- Invalid security selection")
                return

            # Step 5: Send security result (0 = OK)
            security_result = struct.pack('!I', 0)  # 0 = OK
            client_socket.sendall(security_result)
            print(f"[{time.strftime('%H:%M:%S')}] -> Sent security result: OK")

            # Step 6: Receive ClientInit
            client_init = client_socket.recv(1)
            if len(client_init) == 1:
                shared_flag = struct.unpack('B', client_init)[0]
                print(f"[{time.strftime('%H:%M:%S')}] <- Received ClientInit (shared={shared_flag})")
            else:
                print(f"[{time.strftime('%H:%M:%S')}] <- Invalid ClientInit")
                return

            # Step 7: Send ServerInit (framebuffer parameters)
            # Format: width(2) height(2) pixel_format(16) name_length(4) name(variable)
            width = 1024
            height = 768

            # Pixel format (16 bytes):
            # bits_per_pixel(1) depth(1) big_endian(1) true_color(1)
            # red_max(2) green_max(2) blue_max(2)
            # red_shift(1) green_shift(1) blue_shift(1)
            # padding(3)
            pixel_format = struct.pack(
                'BBBB HHH BBB xxx',
                32,  # bits per pixel
                24,  # depth
                0,   # big endian (0 = little endian)
                1,   # true color
                255, # red max
                255, # green max
                255, # blue max
                16,  # red shift
                8,   # green shift
                0,   # blue shift
            )

            desktop_name = b"Horcrux Test VM"
            server_init = struct.pack('!HH', width, height) + pixel_format + \
                         struct.pack('!I', len(desktop_name)) + desktop_name

            client_socket.sendall(server_init)
            print(f"[{time.strftime('%H:%M:%S')}] -> Sent ServerInit ({width}x{height}, '{desktop_name.decode()}')")
            print(f"[{time.strftime('%H:%M:%S')}] VNC handshake complete! Ready for client messages.")

            # Step 8: Keep connection alive and log any messages
            print(f"[{time.strftime('%H:%M:%S')}] Waiting for client messages (Ctrl+C to stop)...")

            while True:
                # Set timeout to allow checking for shutdown
                client_socket.settimeout(1.0)
                try:
                    data = client_socket.recv(1024)
                    if not data:
                        print(f"[{time.strftime('%H:%M:%S')}] Client disconnected")
                        break

                    # Log received message type
                    if len(data) > 0:
                        msg_type = data[0]
                        print(f"[{time.strftime('%H:%M:%S')}] <- Received message type: {msg_type} ({len(data)} bytes)")

                        # Simple responses for common message types
                        if msg_type == 0:  # SetPixelFormat
                            print(f"[{time.strftime('%H:%M:%S')}]    SetPixelFormat message")
                        elif msg_type == 2:  # SetEncodings
                            print(f"[{time.strftime('%H:%M:%S')}]    SetEncodings message")
                        elif msg_type == 3:  # FramebufferUpdateRequest
                            print(f"[{time.strftime('%H:%M:%S')}]    FramebufferUpdateRequest message")
                            # Send empty framebuffer update
                            fb_update = struct.pack('!BxH', 0, 0)  # Type 0, 0 rectangles
                            client_socket.sendall(fb_update)
                            print(f"[{time.strftime('%H:%M:%S')}] -> Sent empty FramebufferUpdate")
                        elif msg_type == 4:  # KeyEvent
                            print(f"[{time.strftime('%H:%M:%S')}]    KeyEvent message")
                        elif msg_type == 5:  # PointerEvent
                            print(f"[{time.strftime('%H:%M:%S')}]    PointerEvent message")
                        elif msg_type == 6:  # ClientCutText
                            print(f"[{time.strftime('%H:%M:%S')}]    ClientCutText message")

                except socket.timeout:
                    continue
                except Exception as e:
                    print(f"[{time.strftime('%H:%M:%S')}] Error receiving data: {e}")
                    break

        except Exception as e:
            print(f"[{time.strftime('%H:%M:%S')}] Error handling client {client_address}: {e}")
        finally:
            client_socket.close()
            print(f"[{time.strftime('%H:%M:%S')}] Connection closed: {client_address}")
            print()

    def stop(self):
        """Stop the mock VNC server"""
        self.running = False

        # Close all client connections
        for client_socket, _ in self.clients:
            try:
                client_socket.close()
            except:
                pass

        # Close server socket
        if hasattr(self, 'server_socket'):
            self.server_socket.close()

        print("\nMock VNC server stopped")

def main():
    """Main entry point"""
    print("=" * 70)
    print("Mock VNC Server for noVNC Testing")
    print("=" * 70)
    print()

    # Parse command line arguments
    host = '127.0.0.1'
    port = 5900

    if len(sys.argv) > 1:
        port = int(sys.argv[1])

    # Create and start server
    server = MockVNCServer(host=host, port=port)

    try:
        server.start()
    except KeyboardInterrupt:
        print("\n\nReceived Ctrl+C, shutting down...")
    except Exception as e:
        print(f"Error: {e}")
    finally:
        server.stop()

if __name__ == '__main__':
    main()
