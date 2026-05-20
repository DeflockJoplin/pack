# Security

## Sensitive data

WiGLE and WDGwars API credentials are stored in `data/config/app.json` on the operator machine. Do not commit the `data/` directory or share that file.

## Network exposure

The default HTTP bind is `127.0.0.1:8787`. If you listen on `0.0.0.0`, hosts on the LAN can reach the API, including `POST /api/shutdown`.

## Reporting issues

Report security vulnerabilities by contacting the maintainers directly. Include steps to reproduce and impact if known.
