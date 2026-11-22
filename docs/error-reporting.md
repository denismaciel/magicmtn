# Error reporting

MagicMtn CLI error reporting is off by default. To opt in, add this line to `~/.config/magicmtn/config`. If you set `MAGICMTN_CONFIG_DIRECTORY`, use the `config` file in that directory.

```toml
error-reporting = true
```

With reporting enabled and the hosted MagicMtn API selected, the CLI sends failure reports to MagicMtn’s Sentry project. Reports contain the CLI version, operation, failure category, and HTTP status when available. The CLI omits raw error messages, credentials, email addresses, user IDs, and organization IDs. MagicMtn also passes your reporting preference to the server when opening hosted sessions.

Set `error-reporting = false` to turn reporting off again. The local CLI workflow can run without connecting to Sentry.
