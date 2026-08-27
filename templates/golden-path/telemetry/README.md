# Telemetry

Tracing is enabled in `wrangler.jsonc` via `observability.enabled`, which uses Cloudflare's native
Workers tracing. No SDK goes in the bundle.

## What you get for free

Handler, `fetch` and binding calls are instrumented automatically, and trace context propagates
over outbound requests as a W3C `traceparent` header.

## What you do not get for free

Business context. No automatic instrumentation can see application semantics, so attributes like
`customer_id`, `plan` or `order_total` have to be written deliberately. These are the attributes
that make a trace answer a question rather than merely record that something happened.

```ts
import { trace } from "./telemetry";

trace.setAttributes({ "app.customer_id": customerId, "app.plan": plan });
```

## Where traces go

Any OTLP endpoint. Honeycomb's free tier covers 20M events/month, which is comfortably more than
a seed-stage service produces, and its high-cardinality querying is what makes the business
attributes above worth writing.
