# Horcrux Mobile UI

Touch-optimized mobile interface for Horcrux built with Rust and Yew.

## Features

- **Touch-Optimized**: Designed specifically for mobile devices with large touch targets
- **Responsive Design**: Works on phones and tablets
- **Bottom Navigation**: Easy thumb-accessible navigation
- **Real-time Updates**: Live dashboard with auto-refresh
- **VM Management**: View, start, and stop VMs from your phone
- **Cluster Monitoring**: Check cluster status and node health
- **Native App Feel**: Smooth animations and gestures

## Pages

- **Dashboard**: Quick overview of cluster status and node metrics
- **VMs**: List and manage virtual machines
- **Cluster**: View cluster nodes and architecture
- **Storage**: Storage management (coming soon)
- **Network**: Network configuration (coming soon)
- **Settings**: User settings and logout

## Building

### Prerequisites

- Rust 1.82+ (for Yew 0.21)
- `trunk` for building WASM apps:
  ```bash
  cargo install trunk
  ```
- `wasm32-unknown-unknown` target:
  ```bash
  rustup target add wasm32-unknown-unknown
  ```

### Development

Run the development server:

```bash
cd horcrux-mobile
trunk serve
```

The app will be available at `http://localhost:8080`

### Production Build

Build for production:

```bash
cd horcrux-mobile
trunk build --release
```

The built app will be in `dist/`.

## Architecture

### Components

- **Card**: Reusable card component for content
- **Header**: Page header with optional back button
- **Loading**: Loading spinner
- **StatusBadge**: Color-coded status indicators

### API Client

The mobile UI communicates with the Horcrux API backend via REST:

- Authentication with JWT tokens
- Token storage in browser LocalStorage
- Automatic token injection in requests

### Routing

Yew Router provides client-side navigation:

- `/` - Dashboard
- `/login` - Login page
- `/vms` - VM list
- `/vms/:id` - VM detail
- `/cluster` - Cluster view
- `/settings` - Settings

## Mobile-First Design

### Touch Targets

- Minimum 44×44px touch targets
- Large, easy-to-tap buttons
- Swipe gestures (planned)

### Performance

- Lightweight WASM bundle
- Efficient DOM updates with Yew
- Optimized for mobile networks

### PWA Support (Planned)

- Offline support
- Add to home screen
- Push notifications

## Compatibility

- iOS 12+ (Safari)
- Android 7+ (Chrome)
- Modern mobile browsers

## License

GPL-3.0
