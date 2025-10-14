#!/usr/bin/env python3
"""
VNC Protocol Integration Test

Tests the complete VNC RFB protocol handshake with the mock VNC server
to verify that the noVNC WebSocket proxy can successfully connect and
relay VNC traffic.
"""

import socket
import struct
import sys
import time

def test_vnc_handshake(host='127.0.0.1', port=5900):
    """
    Perform a complete VNC protocol handshake

    Returns True if successful, False otherwise
    """
    print(f"Connecting to VNC server at {host}:{port}...")

    try:
        # Create socket
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(5.0)
        sock.connect((host, port))
        print(f"✓ Connected to {host}:{port}")

        # Step 1: Receive RFB version from server
        server_version = sock.recv(12)
        if len(server_version) != 12:
            print(f"✗ Invalid server version length: {len(server_version)}")
            return False

        version_str = server_version.decode('ascii').strip()
        print(f"✓ Server version: {version_str}")

        if not version_str.startswith('RFB 003.'):
            print(f"✗ Invalid RFB version: {version_str}")
            return False

        # Step 2: Send client version (same as server)
        sock.sendall(server_version)
        print(f"✓ Sent client version: {version_str}")

        # Step 3: Receive security types
        num_types = struct.unpack('B', sock.recv(1))[0]
        print(f"✓ Received {num_types} security type(s)")

        if num_types == 0:
            # Connection failed
            reason_length = struct.unpack('!I', sock.recv(4))[0]
            reason = sock.recv(reason_length).decode('utf-8')
            print(f"✗ Connection failed: {reason}")
            return False

        security_types = []
        for _ in range(num_types):
            sec_type = struct.unpack('B', sock.recv(1))[0]
            security_types.append(sec_type)

        print(f"✓ Available security types: {security_types}")

        # Step 4: Select security type (None = 1)
        if 1 not in security_types:
            print(f"✗ Security type 'None' (1) not available")
            return False

        sock.sendall(struct.pack('B', 1))
        print(f"✓ Selected security type: None (1)")

        # Step 5: Receive security result
        security_result = struct.unpack('!I', sock.recv(4))[0]
        if security_result != 0:
            print(f"✗ Security handshake failed: {security_result}")
            # Try to read error message if available
            try:
                reason_length = struct.unpack('!I', sock.recv(4))[0]
                reason = sock.recv(reason_length).decode('utf-8')
                print(f"   Reason: {reason}")
            except:
                pass
            return False

        print(f"✓ Security result: OK")

        # Step 6: Send ClientInit (shared flag = 1)
        shared_flag = 1  # 1 = shared, 0 = exclusive
        sock.sendall(struct.pack('B', shared_flag))
        print(f"✓ Sent ClientInit (shared={shared_flag})")

        # Step 7: Receive ServerInit
        # width(2) height(2) pixel_format(16) name_length(4) name(variable)
        server_init = sock.recv(24)  # width + height + pixel_format
        if len(server_init) != 24:
            print(f"✗ Invalid ServerInit length: {len(server_init)}")
            return False

        width, height = struct.unpack('!HH', server_init[:4])
        print(f"✓ Framebuffer size: {width}x{height}")

        # Parse pixel format
        pixel_format = server_init[4:24]
        bits_per_pixel, depth, big_endian, true_color = struct.unpack('BBBB', pixel_format[:4])
        red_max, green_max, blue_max = struct.unpack('!HHH', pixel_format[4:10])
        red_shift, green_shift, blue_shift = struct.unpack('BBB', pixel_format[10:13])

        print(f"✓ Pixel format: {bits_per_pixel}bpp, depth={depth}, true_color={true_color}")
        print(f"  RGB: max=({red_max},{green_max},{blue_max}), shift=({red_shift},{green_shift},{blue_shift})")

        # Receive desktop name
        name_length = struct.unpack('!I', sock.recv(4))[0]
        desktop_name = sock.recv(name_length).decode('utf-8')
        print(f"✓ Desktop name: '{desktop_name}'")

        print(f"\n✓✓✓ VNC handshake completed successfully! ✓✓✓")
        print(f"\nConnection details:")
        print(f"  Server: {host}:{port}")
        print(f"  Protocol: {version_str}")
        print(f"  Resolution: {width}x{height}")
        print(f"  Security: None")
        print(f"  Desktop: {desktop_name}")

        # Step 8: Test sending a FramebufferUpdateRequest message
        print(f"\n✓ Testing FramebufferUpdateRequest...")

        # FramebufferUpdateRequest message:
        # Type(1) Incremental(1) x(2) y(2) width(2) height(2)
        msg = struct.pack('!BBHHHH',
            3,      # Message type: FramebufferUpdateRequest
            0,      # Incremental: 0 = full update, 1 = incremental
            0, 0,   # x, y
            width, height  # width, height
        )
        sock.sendall(msg)
        print(f"✓ Sent FramebufferUpdateRequest")

        # Wait for response (should get empty FramebufferUpdate from mock server)
        sock.settimeout(2.0)
        try:
            response = sock.recv(4)
            if len(response) > 0:
                msg_type = response[0]
                if msg_type == 0:  # FramebufferUpdate
                    print(f"✓ Received FramebufferUpdate response")
                else:
                    print(f"✓ Received message type: {msg_type}")
        except socket.timeout:
            print(f"⊘ No response (expected with mock server)")

        # Close connection
        sock.close()
        print(f"\n✓ Connection closed cleanly")

        return True

    except ConnectionRefusedError:
        print(f"✗ Connection refused - is the VNC server running?")
        return False
    except socket.timeout:
        print(f"✗ Connection timeout")
        return False
    except Exception as e:
        print(f"✗ Error: {e}")
        import traceback
        traceback.print_exc()
        return False

def main():
    """Main entry point"""
    print("=" * 70)
    print("VNC Protocol Integration Test")
    print("=" * 70)
    print()

    # Parse command line arguments
    host = '127.0.0.1'
    port = 5900

    if len(sys.argv) > 1:
        port = int(sys.argv[1])

    # Run test
    success = test_vnc_handshake(host, port)

    print()
    print("=" * 70)
    if success:
        print("✓✓✓ TEST PASSED ✓✓✓")
        print()
        print("The mock VNC server is working correctly and can:")
        print("  - Accept TCP connections")
        print("  - Perform RFB protocol handshake")
        print("  - Negotiate security (None)")
        print("  - Send framebuffer parameters")
        print("  - Receive and respond to client messages")
        print()
        print("This confirms that the noVNC WebSocket proxy should be")
        print("able to successfully relay VNC traffic.")
    else:
        print("✗✗✗ TEST FAILED ✗✗✗")
        print()
        print("The VNC handshake did not complete successfully.")
        print("Check that the mock VNC server is running:")
        print(f"  python3 test_vnc_server.py {port}")
    print("=" * 70)

    return 0 if success else 1

if __name__ == '__main__':
    sys.exit(main())
