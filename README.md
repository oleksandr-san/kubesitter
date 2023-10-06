## Uniskai Agent

Uniskai Agent brings the platform features to Kubernetes clusters without compromising security.

Kubesitter defines the `SchedulePoicy` CRD which allows to define a schedule for a set of namespaces.
The schedule is defined by a set of `WorkTime` objects. The `SchedulePolicy` object is applied to namespaces that match the `NamespaceSelector`. The `SchedulePolicy` object can be suspended by setting the `suspend` field to `true`.

## Installation

### CRD
Apply the CRD from [cached file](yaml/crd.yaml), or pipe it from `crdgen` to pickup schema changes:

```sh
cargo run -p agent --bin crdgen | kubectl apply -f -
```

### Controller

Install the controller via `helm` by setting your preferred settings. For defaults:

```sh
helm template charts/uniskai-agent | kubectl apply -f -
```

### Metrics

Metrics is available on `/metrics` and a `ServiceMonitor` is configurable from the chart:

```sh
helm template charts/uniskai-agent --set serviceMonitor.enabled=true | kubectl apply -f -
```

## Running

### Locally

```sh
cargo run
```

### In-cluster
For prebuilt, edit the [chart values](./charts/uniskai-agent/values.yaml) or [snapshotted yaml](./yaml/deployment.yaml) and apply as you see fit (like above).

To develop by building and deploying the image quickly, we recommend using [tilt](https://tilt.dev/), via `tilt up` instead.

## Usage
In either of the run scenarios, your app is listening on port `8080`, and it will observe `Document` events.

Try some of:

```sh
kubectl apply -f yaml/instance-lorem.yaml
kubectl delete doc lorem
kubectl edit doc lorem # change hidden
```

The reconciler will run and write the status object on every change. You should see results in the logs of the pod, or on the `.status` object outputs of `kubectl get doc -oyaml`.

### Webapp output
The sample web server exposes some example metrics and debug information you can inspect with `curl`.

```sh
$ kubectl apply -f yaml/instance-lorem.yaml
$ curl 0.0.0.0:8080/metrics
# HELP controller_reconcile_duration_seconds The duration of reconcile to complete in seconds
# TYPE controller_reconcile_duration_seconds histogram
controller_reconcile_duration_seconds_bucket{le="0.01"} 1
controller_reconcile_duration_seconds_bucket{le="0.1"} 1
controller_reconcile_duration_seconds_bucket{le="0.25"} 1
controller_reconcile_duration_seconds_bucket{le="0.5"} 1
controller_reconcile_duration_seconds_bucket{le="1"} 1
controller_reconcile_duration_seconds_bucket{le="5"} 1
controller_reconcile_duration_seconds_bucket{le="15"} 1
controller_reconcile_duration_seconds_bucket{le="60"} 1
controller_reconcile_duration_seconds_bucket{le="+Inf"} 1
controller_reconcile_duration_seconds_sum 0.013
controller_reconcile_duration_seconds_count 1
# HELP controller_reconciliation_errors_total reconciliation errors
# TYPE controller_reconciliation_errors_total counter
controller_reconciliation_errors_total 0
# HELP controller_reconciliations_total reconciliations
# TYPE controller_reconciliations_total counter
controller_reconciliations_total 1
```

The metrics will be scraped by prometheus if you setup a`ServiceMonitor` for it.
