# Cloudflare: the tunnel and `tests.famat.org`

```text
browser ─► Cloudflare edge ─► tunnel "famat" ─► famat-cloudflared ─► http://app:8080
             famat.rconde.xyz                     (a container in the       (famat-backend)
             tests.famat.org                       Compose project famat)
               │
               └ famat.rconde.xyz: public hostname on the tunnel
                 tests.famat.org: Cloudflare for SaaS custom hostname, fallback origin = famat.rconde.xyz
```

Cloudflare terminates HTTPS. cloudflared dials out, so the server opens no ports.

**This is a tunnel of its own.** The server already runs a host `cloudflared` service with another tunnel. Leave that service, its `/etc/cloudflared/` and its tunnel's routes exactly as they are. Don't add the test bank's hostnames to it, and never run `cloudflared service install` on the host. The test bank's tunnel is run by the `famat-cloudflared` container, with the token from `/etc/famat/secrets/tunnel.env`.

Two rules hold for every request:

- **The Host header stays the visitor's hostname.** Never set an HTTP Host Header (`httpHostHeader`) or Origin Server Name override on the tunnel's routes. The app refuses any write whose `Origin` doesn't match `Host`, so a rewritten Host breaks every login and Admin form on the hostname that wasn't rewritten.
- **`CF-Connecting-IP` is the client.** The login rate limit trusts it, which is safe only because the tunnel is the only way to the app.

Cloudflare also refuses any request over 100 MB on the Free and Pro plans, with its own error page. The app keeps uploads under that: a file can be at most 90 MB, pages with JavaScript upload each file in its own request, and the server refuses a no-JavaScript post of several files over about 95 MB with its own message (README, Adding Tests).

## 1. The tunnel for `famat.rconde.xyz`

In the Cloudflare dashboard, open Zero Trust → Networks → Tunnels:

1. Select **Create a tunnel**, then **Cloudflared**. Name it `famat`. The existing tunnel in the list belongs to the host's `cloudflared`; don't open or edit it.
2. On the install page, pick **Docker**. Copy the token, the long `eyJ…` string after `--token`. Don't run the install command anywhere; the `famat-cloudflared` container runs cloudflared. On the server, run `deploy/create-secrets.sh` and paste the token alone when asked ([server-setup.md](server-setup.md), step 4).
3. Add a **public hostname** (in newer dashboards, a **published application route**):
   - Subdomain `famat`, domain `rconde.xyz`, path empty.
   - Service type `HTTP`, URL `app:8080`. `app` is the app's name on the Compose network, which cloudflared shares; `localhost` would be cloudflared's own container.
   - Additional application settings → HTTP Settings: leave **HTTP Host Header** empty.

   Cloudflare creates the proxied DNS record `famat.rconde.xyz CNAME <tunnel-id>.cfargotunnel.com`. If it says a record already exists, check what it points to first: if it is the other tunnel's, stop and decide; don't overwrite it.
4. Once the containers are running, the tunnel shows **Healthy**, and `docker ps` shows `famat-cloudflared` as `(healthy)`. Check that `https://famat.rconde.xyz/healthz` answers `ok`.

Note the **tunnel ID**, a UUID shown on the tunnel's page, and the **account ID** from the dashboard URL or the zone's Overview page. Part 2 needs both.

## 2. `tests.famat.org` through Cloudflare for SaaS

`famat.org` is in someone else's DNS, and a plain cross-account CNAME to a proxied record gives error 1014. So `tests.famat.org` is added as a **custom hostname** on the `rconde.xyz` zone. The free plan includes 100.

### 2a. Fallback origin (once)

In the `rconde.xyz` zone, open SSL/TLS → Custom Hostnames:

1. If asked, enable Cloudflare for SaaS (free tier).
2. Under **Fallback Origin**, enter `famat.rconde.xyz` and select **Add Fallback Origin**. It becomes **Active** in a minute or two. This record is the tunnel's own proxied CNAME, so custom hostname traffic goes to the tunnel.

### 2b. Route the custom hostname in the tunnel

cloudflared picks a route by the request's hostname. Requests for `tests.famat.org` keep that hostname, so they don't match the `famat.rconde.xyz` route, and the dashboard's catch-all answers 404. The dashboard only offers hostnames in your own zones, so set the tunnel's routes through the API. You need an API token with **Account → Cloudflare Tunnel → Edit**.

```sh
ACCOUNT_ID=<account id>
TUNNEL_ID=<tunnel id>
CF_API_TOKEN=<api token>     # not saved anywhere; delete the token afterwards if it was made for this

curl -fsS -X PUT \
  "https://api.cloudflare.com/client/v4/accounts/$ACCOUNT_ID/cfd_tunnel/$TUNNEL_ID/configurations" \
  -H "Authorization: Bearer $CF_API_TOKEN" -H "Content-Type: application/json" \
  --data '{"config":{"ingress":[
    {"hostname":"famat.rconde.xyz","service":"http://app:8080","originRequest":{}},
    {"hostname":"tests.famat.org","service":"http://app:8080","originRequest":{}},
    {"service":"http_status:404"}
  ]}}'
```

This replaces the `famat` tunnel's whole route list, including the route from part 1, so keep both hostnames in it. Check `TUNNEL_ID` is the `famat` tunnel's, not the other tunnel's: this call would replace that one's routes just the same. Read it back with the same URL and `GET`. Within a few seconds, `docker logs famat-cloudflared` shows `Updated to new configuration`.

If Cloudflare rejects the `tests.famat.org` rule, make the last rule `{"service":"http://app:8080"}` instead. Every hostname that reaches the `famat` tunnel then goes to the app.

### 2c. Add the custom hostname

Still in SSL/TLS → Custom Hostnames, select **Add Custom Hostname**:

- Hostname: `tests.famat.org`
- Minimum TLS version: 1.2
- Certificate validation method: **TXT**. With TXT, the certificate is issued before the CNAME moves, so the switch has no HTTPS gap.
- No custom origin server: it uses the fallback origin.

Open the new hostname's details. Cloudflare shows the values to send to FAMAT.

### 2d. The records FAMAT's DNS admin adds

Copy the values exactly. The tokens are made for this hostname, so they can't be written down in advance.

| Type | Name | Value | Why |
|---|---|---|---|
| `TXT` | `_cf-custom-hostname.tests.famat.org` | the **Hostname pre-validation TXT value** (a UUID, `ownership_verification` in the API) | proves famat.org agrees to this |
| `TXT` | `_acme-challenge.tests.famat.org` | the **Certificate validation TXT value** | lets the CA issue the certificate |
| `CNAME` | `tests.famat.org` | `famat.rconde.xyz` | sends the traffic to Cloudflare |

- The CNAME's target is the fallback origin itself. That's enough for one custom hostname. If more custom hostnames come later, create a dedicated proxied CNAME target first (e.g. `customers.rconde.xyz CNAME famat.rconde.xyz`) and point the new ones at that.
- If FAMAT's DNS provider is itself Cloudflare, the CNAME must stay **DNS only** (grey cloud) on their side.
- A CAA record on `famat.org` must allow the CA Cloudflare picks: `letsencrypt.org`, `pki.goog` or `ssl.com`. With no CAA records, any CA is allowed.

Message for FAMAT's DNS admin. Fill in the two values:

> Please add these three records to famat.org:
>
> - `_cf-custom-hostname.tests` TXT `<uuid>`
> - `_acme-challenge.tests` TXT `<token>`
> - `tests` CNAME `famat.rconde.xyz`
>
> The TXT records may be added first; the CNAME switches tests.famat.org to the new site.

### 2e. Wait for Active

The custom hostname's **Status** and **Certificate status** both reach **Active**. This usually takes minutes after the TXT records appear, and at most a few hours. Check from anywhere:

```sh
dig +short TXT _acme-challenge.tests.famat.org
dig +short CNAME tests.famat.org
curl -sS https://tests.famat.org/healthz        # ok
curl -sSI https://tests.famat.org/ | grep -i '^server'   # cloudflare
```

Then log in on `https://tests.famat.org/login` to check the Origin check accepts it, and add the Better Stack monitor for `https://tests.famat.org/healthz` ([server-setup.md](server-setup.md), step 9).

**Renewal:** the certificate renews every ~90 days. Once the CNAME is in place, Cloudflare may renew it over HTTP without new TXT values. If the dashboard ever shows a pending renewal with new TXT values, send FAMAT the new `_acme-challenge` value.
