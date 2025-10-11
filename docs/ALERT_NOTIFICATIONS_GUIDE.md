# Alert Notifications Configuration Guide

**Version**: 0.1.0
**Last Updated**: 2025-10-10
**Status**: Production Ready

---

## Overview

Horcrux supports three notification channels for alerts:
1. **Email** - Native SMTP with TLS support
2. **Webhooks** - HTTP/HTTPS callbacks
3. **Syslog** - System logging integration

This guide covers configuration and best practices for the newly enhanced native notification system.

---

## Email Notifications (SMTP)

### Features ✨

- ✅ Native SMTP implementation (no `mail` command required)
- ✅ TLS/SSL encryption via rustls
- ✅ SMTP authentication support
- ✅ Multiple recipients
- ✅ Configurable ports and servers
- ✅ Both secure and plain SMTP modes

### Basic Configuration

```toml
# /etc/horcrux/config.toml

[alerts.email]
enabled = true
smtp_server = "smtp.gmail.com"
smtp_port = 587
use_tls = true
from_address = "horcrux-alerts@example.com"
to_addresses = ["admin@example.com", "ops-team@example.com"]
username = "horcrux-alerts@example.com"
password = "your-app-password"
```

### Provider-Specific Examples

#### Gmail

```toml
[alerts.email]
smtp_server = "smtp.gmail.com"
smtp_port = 587
use_tls = true
username = "your-email@gmail.com"
password = "app-specific-password"  # Generate at https://myaccount.google.com/apppasswords
```

**Important**: Use App Passwords, not your regular Gmail password!

#### Office 365 / Outlook

```toml
[alerts.email]
smtp_server = "smtp.office365.com"
smtp_port = 587
use_tls = true
username = "alerts@yourdomain.com"
password = "your-password"
```

#### SendGrid

```toml
[alerts.email]
smtp_server = "smtp.sendgrid.net"
smtp_port = 587
use_tls = true
username = "apikey"
password = "SG.your-sendgrid-api-key"
```

#### AWS SES

```toml
[alerts.email]
smtp_server = "email-smtp.us-east-1.amazonaws.com"
smtp_port = 587
use_tls = true
username = "AKIAIOSFODNN7EXAMPLE"
password = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
```

#### Mailgun

```toml
[alerts.email]
smtp_server = "smtp.mailgun.org"
smtp_port = 587
use_tls = true
username = "postmaster@mg.yourdomain.com"
password = "your-mailgun-password"
```

#### Self-Hosted (Postfix/Exim)

```toml
# With TLS
[alerts.email]
smtp_server = "mail.yourdomain.com"
smtp_port = 587
use_tls = true
username = "alerts"
password = "secure-password"

# Without TLS (internal networks only)
[alerts.email]
smtp_server = "localhost"
smtp_port = 25
use_tls = false
# No authentication required for local relay
```

### Email Format

Alert emails include:
- **Subject**: `[SEVERITY] RuleName - Target`
- **Body**:
  ```
  Alert: RuleName
  Target: vm-100
  Severity: Critical
  Status: Firing

  Message: CPU usage exceeded threshold

  Metric Value: 95.2
  Threshold: 80.0
  Fired At: Mon, 10 Oct 2025 20:30:15 +0000

  --
  Horcrux Alert System
  ```

### Testing Email Configuration

```bash
# Via CLI
horcrux-cli alerts test-notification \
  --type email \
  --to admin@example.com \
  --subject "Test Alert" \
  --message "This is a test notification"

# Via API
curl -X POST http://localhost:8006/api/alerts/test \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "type": "email",
    "config": {
      "smtp_server": "smtp.gmail.com",
      "smtp_port": 587,
      "use_tls": true,
      "from_address": "test@example.com",
      "to_addresses": ["admin@example.com"],
      "username": "test@example.com",
      "password": "app-password"
    }
  }'
```

### Troubleshooting Email

**Problem**: "Failed to connect to SMTP server"
```bash
# Solution: Check network connectivity
telnet smtp.gmail.com 587

# Verify TLS support
openssl s_client -connect smtp.gmail.com:587 -starttls smtp
```

**Problem**: "Authentication failed"
```bash
# Solution: Verify credentials
# For Gmail: Use App Passwords
# For others: Check username/password format
```

**Problem**: "Invalid from address"
```bash
# Solution: Ensure from_address is valid email format
# Must include both local and domain parts
from_address = "alerts@example.com"  # ✅ Correct
from_address = "alerts"               # ❌ Wrong
```

---

## Webhook Notifications (HTTP)

### Features ✨

- ✅ Native HTTP client (no `curl` required)
- ✅ Support for GET, POST, PUT, PATCH, DELETE
- ✅ Custom headers
- ✅ Bearer token authentication
- ✅ JSON payload
- ✅ Detailed error responses

### Basic Configuration

```toml
[alerts.webhook]
enabled = true
url = "https://hooks.example.com/alerts"
method = "POST"
auth_token = "your-secret-token"

# Optional custom headers
[[alerts.webhook.headers]]
name = "X-Custom-Header"
value = "custom-value"
```

### Provider-Specific Examples

#### Slack

```toml
[alerts.webhook]
url = "https://hooks.slack.com/services/T00000000/B00000000/XXXXXXXXXXXXXXXXXXXX"
method = "POST"
# No auth_token needed (URL contains secret)
```

**Payload sent to Slack**:
```json
{
  "alert_id": "alert-123",
  "rule_id": "rule-456",
  "rule_name": "high-cpu",
  "severity": "Critical",
  "status": "Firing",
  "message": "CPU usage exceeded threshold",
  "target": "vm-100",
  "metric_value": 95.2,
  "threshold": 80.0,
  "fired_at": 1696953600
}
```

**Slack Formatting** (use Slack workflow builder to format):
```
🚨 *[Critical] high-cpu - vm-100*

CPU usage exceeded threshold

Metric: 95.2 / 80.0
Time: <timestamp>
```

#### Microsoft Teams

```toml
[alerts.webhook]
url = "https://yourcompany.webhook.office.com/webhookb2/GUID/IncomingWebhook/GUID"
method = "POST"
```

#### PagerDuty

```toml
[alerts.webhook]
url = "https://events.pagerduty.com/v2/enqueue"
method = "POST"
auth_token = "your-integration-key"

[[alerts.webhook.headers]]
name = "Content-Type"
value = "application/json"
```

#### Discord

```toml
[alerts.webhook]
url = "https://discord.com/api/webhooks/1234567890/abcdefghijklmnopqrstuvwxyz"
method = "POST"
```

#### Custom Webhook Server

```toml
[alerts.webhook]
url = "https://monitoring.yourdomain.com/api/alerts"
method = "POST"
auth_token = "Bearer your-jwt-token"

[[alerts.webhook.headers]]
name = "X-API-Key"
value = "your-api-key"

[[alerts.webhook.headers]]
name = "X-Environment"
value = "production"
```

### Webhook Payload Format

```json
{
  "alert_id": "uuid-v4",
  "rule_id": "rule-123",
  "rule_name": "high-cpu-usage",
  "severity": "Critical|Warning|Info",
  "status": "Firing|Resolved",
  "message": "Human-readable description",
  "target": "vm-100",
  "metric_value": 95.2,
  "threshold": 80.0,
  "fired_at": 1696953600
}
```

### HTTP Methods Supported

- **POST** - Most common, create new alert
- **PUT** - Update existing alert
- **PATCH** - Partial update
- **DELETE** - Remove/acknowledge alert
- **GET** - Query (less common for alerts)

### Testing Webhook Configuration

```bash
# Test webhook locally
horcrux-cli alerts test-notification \
  --type webhook \
  --url https://webhook.site/your-unique-id \
  --method POST

# With authentication
horcrux-cli alerts test-notification \
  --type webhook \
  --url https://api.example.com/alerts \
  --method POST \
  --token "your-secret-token"
```

### Troubleshooting Webhooks

**Problem**: "Connection refused"
```bash
# Solution: Check URL and network connectivity
curl -v https://hooks.example.com/alerts
```

**Problem**: "401 Unauthorized"
```bash
# Solution: Verify auth_token
# Ensure Bearer token is correct
auth_token = "your-token"  # Not "Bearer your-token"
```

**Problem**: "Webhook request failed with status 400"
```bash
# Solution: Check payload format
# Some services expect specific JSON structure
# Use webhook.site to inspect actual payload
```

---

## Syslog Notifications

### Features

- ✅ Standard syslog integration
- ✅ Priority-based routing (crit/warning/info)
- ✅ Tagged messages for filtering

### Configuration

```toml
[alerts.syslog]
enabled = true
facility = "user"
tag = "horcrux-alerts"
```

### Log Format

```
Oct 10 20:30:15 hostname horcrux-alerts[PID]: [HORCRUX-ALERT] high-cpu - vm-100: CPU usage exceeded threshold
```

### Viewing Syslog Alerts

```bash
# View all Horcrux alerts
journalctl -t horcrux-alerts -f

# Filter by priority
journalctl -t horcrux-alerts -p crit

# Specific time range
journalctl -t horcrux-alerts --since "1 hour ago"
```

---

## Alert Rules

### Creating Alert Rules

```bash
# CPU alert
horcrux-cli alerts rule create \
  --name high-cpu \
  --metric cpu_usage \
  --comparison greater_than \
  --threshold 80 \
  --severity warning \
  --target vm-* \
  --notification email:admin@example.com

# Memory alert
horcrux-cli alerts rule create \
  --name high-memory \
  --metric memory_usage \
  --comparison greater_than \
  --threshold 90 \
  --severity critical \
  --target vm-* \
  --notification webhook:slack,email:admin@example.com

# Disk alert
horcrux-cli alerts rule create \
  --name disk-full \
  --metric disk_usage \
  --comparison greater_than \
  --threshold 85 \
  --severity warning \
  --notification syslog
```

### Alert Severities

| Severity | Email Priority | Webhook Color | Syslog Priority |
|----------|----------------|---------------|-----------------|
| Critical | High | Red | crit |
| Warning | Medium | Yellow | warning |
| Info | Low | Blue | info |

---

## Advanced Configuration

### Multiple Notification Channels

```toml
# Send critical alerts via email and webhook
[[alerts.channels]]
name = "critical-team"
type = "email"
enabled = true
min_severity = "Critical"
config = { smtp_server = "smtp.gmail.com", smtp_port = 587, ... }

[[alerts.channels]]
name = "slack-critical"
type = "webhook"
enabled = true
min_severity = "Critical"
config = { url = "https://hooks.slack.com/...", method = "POST" }

# Send all alerts to syslog
[[alerts.channels]]
name = "syslog-all"
type = "syslog"
enabled = true
min_severity = "Info"
```

### Rate Limiting

```toml
[alerts]
# Prevent alert spam
rate_limit_window = 300  # 5 minutes
rate_limit_count = 10    # Max 10 alerts per window

# Alert grouping
group_by = ["target", "rule_name"]
group_interval = 60      # Group alerts within 1 minute
```

### Alert Templates

Custom email templates:

```toml
[alerts.email_template]
subject = "[{{severity}}] {{rule_name}} on {{target}}"
body = """
⚠️ Alert: {{rule_name}}
📍 Target: {{target}}
📊 Metric: {{metric_value}} / {{threshold}}
⏰ Time: {{fired_at}}

{{message}}

View details: https://horcrux.example.com/alerts/{{alert_id}}
"""
```

---

## Security Best Practices

### 1. Credential Storage

```bash
# Store credentials securely (not in config file)
export HORCRUX_SMTP_PASSWORD="your-password"
export HORCRUX_WEBHOOK_TOKEN="your-token"

# Reference in config
[alerts.email]
password = "${HORCRUX_SMTP_PASSWORD}"

[alerts.webhook]
auth_token = "${HORCRUX_WEBHOOK_TOKEN}"
```

### 2. TLS Enforcement

```toml
# Always use TLS for SMTP
[alerts.email]
use_tls = true
smtp_port = 587  # STARTTLS
# OR
smtp_port = 465  # SMTPS

# Always use HTTPS for webhooks
[alerts.webhook]
url = "https://..."  # ✅ Secure
# NOT http://...     # ❌ Insecure
```

### 3. Webhook Authentication

```toml
# Use authentication headers
[alerts.webhook]
auth_token = "secret-token"

# OR custom headers
[[alerts.webhook.headers]]
name = "X-API-Key"
value = "your-api-key"

# Verify webhooks on receiver side
# Check auth_token or custom headers
```

### 4. Minimize Sensitive Data

```toml
# Don't include sensitive info in alerts
[alerts]
include_vm_config = false
include_storage_paths = false
redact_ip_addresses = true
```

---

## Performance Considerations

### Email Sending

- **Async Execution**: Emails sent via `tokio::task::spawn_blocking`
- **No Connection Pooling**: New connection per email (fine for low-volume alerts)
- **Recommendation**: For high-volume, consider external email service

### Webhook Sending

- **Fully Async**: Uses reqwest async HTTP client
- **Connection Pooling**: Automatic via reqwest
- **Timeout**: 30 seconds default
- **Retry**: Not implemented (single attempt)

### Recommendations

```toml
[alerts.performance]
# Batch alerts to reduce notification frequency
batch_window = 60           # Wait 60s before sending
batch_max_size = 10         # Send after 10 alerts

# Async processing
async_notifications = true  # Don't block alert evaluation
queue_size = 1000           # Buffer up to 1000 notifications
```

---

## Monitoring Notifications

### Metrics

```bash
# Prometheus metrics
horcrux_alerts_sent_total{channel="email",status="success"} 42
horcrux_alerts_sent_total{channel="email",status="failure"} 2
horcrux_alerts_sent_total{channel="webhook",status="success"} 38

horcrux_notification_duration_seconds{channel="email"} 0.245
horcrux_notification_duration_seconds{channel="webhook"} 0.082
```

### Logs

```bash
# View notification logs
journalctl -u horcrux | grep "notification"

# Examples:
# INFO Sent email notification to admin@example.com for alert: alert-123
# INFO Sent webhook notification for alert: alert-456 to https://hooks.slack.com/...
# ERROR Failed to send email to admin@example.com: Connection timeout
```

---

## Migration from Legacy

### From `mail` Command

**Before** (legacy):
```bash
echo "Alert message" | mail -s "Alert" admin@example.com
```

**After** (native SMTP):
```toml
[alerts.email]
smtp_server = "localhost"
smtp_port = 25
use_tls = false
from_address = "horcrux@localhost"
to_addresses = ["admin@example.com"]
```

### From `curl` Command

**Before** (legacy):
```bash
curl -X POST https://hooks.slack.com/... \
  -d '{"text":"Alert message"}'
```

**After** (native HTTP):
```toml
[alerts.webhook]
url = "https://hooks.slack.com/..."
method = "POST"
```

**Migration is automatic** - Old config continues to work as fallback!

---

## Examples

### Complete Email + Webhook Setup

```toml
# /etc/horcrux/alerts.toml

[alerts]
enabled = true

# Email for critical alerts
[[alerts.channels]]
name = "email-critical"
type = "email"
enabled = true
min_severity = "Critical"

[alerts.channels.config]
smtp_server = "smtp.gmail.com"
smtp_port = 587
use_tls = true
from_address = "horcrux-alerts@example.com"
to_addresses = ["admin@example.com", "oncall@example.com"]
username = "horcrux-alerts@example.com"
password = "${SMTP_PASSWORD}"

# Slack for all alerts
[[alerts.channels]]
name = "slack-all"
type = "webhook"
enabled = true
min_severity = "Info"

[alerts.channels.config]
url = "https://hooks.slack.com/services/YOUR/WEBHOOK/URL"
method = "POST"

# Syslog for audit trail
[[alerts.channels]]
name = "syslog-audit"
type = "syslog"
enabled = true
min_severity = "Info"
```

### Alert Rule Examples

```bash
# VM CPU alert
horcrux-cli alerts rule create \
  --name vm-high-cpu \
  --metric cpu_usage \
  --threshold 80 \
  --severity warning \
  --target "vm-*" \
  --notification email-critical,slack-all

# Node memory alert
horcrux-cli alerts rule create \
  --name node-high-memory \
  --metric node_memory_usage \
  --threshold 90 \
  --severity critical \
  --target "node-*" \
  --notification email-critical,slack-all,syslog-audit

# Storage capacity alert
horcrux-cli alerts rule create \
  --name storage-low \
  --metric storage_available \
  --comparison less_than \
  --threshold 10 \
  --severity critical \
  --notification email-critical
```

---

## Troubleshooting Checklist

- [ ] SMTP server reachable (telnet smtp.server.com 587)
- [ ] SMTP credentials valid
- [ ] TLS enabled if required by server
- [ ] From address valid email format
- [ ] To addresses valid email format
- [ ] Webhook URL accessible (curl test)
- [ ] Webhook authentication correct
- [ ] JSON payload format accepted by receiver
- [ ] Firewall allows outbound connections
- [ ] DNS resolution working
- [ ] Logs checked for errors (journalctl -u horcrux)

---

## Support

- **Documentation**: https://docs.horcrux.io/alerts
- **Examples**: https://github.com/horcrux/examples/alerts
- **Community**: https://community.horcrux.io
- **Issues**: https://github.com/horcrux/issues

---

*Guide Version: 1.0.0*
*Last Updated: 2025-10-10*
*Horcrux Version: 0.1.0+*
