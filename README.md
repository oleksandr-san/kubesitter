## Uniskai Agent

Uniskai Agent brings the platform features to Kubernetes clusters without compromising security.

Kubesitter defines the `SchedulePoicy` CRD which allows to define a schedule for a set of namespaces.
The schedule is defined by a set of `WorkTime` objects. The `SchedulePolicy` object is applied to namespaces that match the `NamespaceSelector`. The `SchedulePolicy` object can be suspended by setting the `suspend` field to `true`.

## Dev Image release

1. Build the docker image
```sh
TAG=<TAG>
docker build -t uniskai-agent:$TAG .
```

2. Prepare Azure image and charts
```sh
# Publish the docker image to Azure ACR
az login
az acr login --name uniskaidevoa
docker tag uniskai-agent:$TAG uniskaidevoa.azurecr.io/uniskai-agent:$TAG
docker push uniskaidevoa.azurecr.io/uniskai-agent:$TAG

# Prepare Azure deployment manifest
helm template charts/uniskai-agent --set version=$TAG  --set image.repository=uniskaidevoa.azurecr.io/uniskai-agent > ./yaml/deployment-azure.yaml
helm template charts/uniskai-agent --set version=$TAG  --set image.repository=uniskaidevoa.azurecr.io/uniskai-agent --set vpnEnabled=true > ./yaml/deployment-azure-vpn.yaml

# Deploy Azure deployment manifest
aws s3 cp --acl public-read ./yaml/deployment-azure.yaml s3://uniskai-dev-templates/kubernetes-agent/deployment-azure.yaml
aws s3 cp --acl public-read ./yaml/deployment-azure-vpn.yaml s3://uniskai-dev-templates/kubernetes-agent/deployment-azure-vpn.yaml
```

3. Prepare AWS image and charts
```sh
# Publish the docker image to AWS ECR
aws ecr get-login-password --region eu-west-1 | docker login --username AWS --password-stdin 199042988758.dkr.ecr.eu-west-1.amazonaws.com
docker tag uniskai-agent:$TAG 199042988758.dkr.ecr.eu-west-1.amazonaws.com/uniskai-agent-dev:$TAG
docker push 199042988758.dkr.ecr.eu-west-1.amazonaws.com/uniskai-agent-dev:$TAG

# Prepare AWS deployment manifest
helm template charts/uniskai-agent --set version=$TAG --set image.repository=199042988758.dkr.ecr.eu-west-1.amazonaws.com/uniskai-agent-dev > ./yaml/deployment-aws.yaml
helm template charts/uniskai-agent --set version=$TAG --set image.repository=199042988758.dkr.ecr.eu-west-1.amazonaws.com/uniskai-agent-dev --set vpnEnabled=true > ./yaml/deployment-aws-vpn.yaml

# Deploy AWS deployment manifest
aws s3 cp --acl public-read ./yaml/deployment-aws.yaml s3://uniskai-dev-templates/kubernetes-agent/deployment-aws.yaml
aws s3 cp --acl public-read ./yaml/deployment-aws-vpn.yaml s3://uniskai-dev-templates/kubernetes-agent/deployment-aws-vpn.yaml
```

4. Update and deploy CRD manifest
```sh
cargo run --bin crdgen -p kubesitter > ./yaml/crd.yaml
aws s3 cp --acl public-read ./yaml/crd.yaml s3://uniskai-dev-templates/kubernetes-agent/crd.yaml
```

## Installation

### CRD
Apply the CRD from [cached file](yaml/crd.yaml), or pipe it from `crdgen` to pickup schema changes:

```sh
cargo run --bin crdgen | kubectl apply -f -
```

### Controller

Install the controller via `helm` by setting your preferred settings. For defaults:

```sh
helm template charts/uniskai-agent | kubectl apply -f -
kubectl wait --for=condition=available deploy/uniskai-agent --timeout=30s
kubectl port-forward service/uniskai-agent 8080:80
```

### Opentelemetry

Build and run with `telemetry` feature, or configure it via `helm`:

```sh
helm template charts/uniskai-agent --set tracing.enabled=true | kubectl apply -f -
```

This requires an opentelemetry collector in your cluster. [Tempo](https://github.com/grafana/helm-charts/tree/main/charts/tempo) / [opentelemetry-operator](https://github.com/open-telemetry/opentelemetry-helm-charts/tree/main/charts/opentelemetry-operator) / [grafana agent](https://github.com/grafana/helm-charts/tree/main/charts/agent-operator) should all work out of the box. If your collector does not support grpc otlp you need to change the exporter in [`telemetry.rs`](./src/telemetry.rs).

Note that the [images are pushed either with or without the telemetry feature](https://hub.docker.com/r/clux/controller/tags/) depending on whether the tag includes `otel`.

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

or, with optional telemetry:

```sh
OPENTELEMETRY_ENDPOINT_URL=https://0.0.0.0:55680 RUST_LOG=info,kube=trace,controller=debug cargo run --features=telemetry
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
$ curl 0.0.0.0:8080/
{"last_event":"2019-07-17T22:31:37.591320068Z"}
```

The metrics will be scraped by prometheus if you setup a`ServiceMonitor` for it.

### Events
The example `reconciler` only checks the `.spec.hidden` bool. If it does, it updates the `.status` object to reflect whether or not the instance `is_hidden`. It also sends a Kubernetes event associated with the controller. It is visible at the bottom of `kubectl describe doc samuel`.

To extend this controller for a real-world setting. Consider looking at the [kube.rs controller guide](https://kube.rs/controllers/intro/).
