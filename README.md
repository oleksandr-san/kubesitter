## Kubesitter

Kubesitter - Kubernetes tool to scale resources up or down based on scheduling policies.

Kubesitter works as a Kubernetes operator - an agent that you install in your cluster that performs scheduling resources automatically. The operator regularly checks policies for desired resource state, detects the current state of selected resources, and applies corrective patches to resources when needed. This is useful for development environments that are used only for a period of time within a day.

Scheduling policies are configured directly via CRD (custom resource definition) in Kubernetes API.

Each policy has:
- title
- suspend - suspension flag to quickly disable the policy
- namespace selector to match Kubernetes namespaces by label or name
- schedule consisting of a list of work periods
- timezone
- list of assignments to start/stop resources at specific times (takes precedence over schedule)

Here is an example of a schedule policy:
```yaml
apiVersion: api.profisealabs.com/v1alpha
kind: SchedulePolicy
metadata:
  name: example
  labels:
    purpose: example
spec:
  # The policy can be suspended by setting the suspend field to true.
  suspend: true
  title: "An example policy"

  namespaceSelector:
    # Either matchLabels, matchExpressions or matchNames can be used
    # matchLabels:
    #   provider: kubernetes-sample-apps
    # matchExpressions:
    #   - key: project
    #     operator: in
    #     values:
    #       - emojivoto
    #       - bookinfo
    # Match namespace names using regular expressions
    matchNames:
      - bookin.*
      - emojivoto

  timeZone: "Europe/Kyiv"

  assignments:
    - type: work  # or sleep or skip
      from: "2023-09-14T00:00:00"
      to: "2023-09-14T22:59:59"
      resourceReferences:
        - name: emojivoto

  schedule:   
    # A set of WorkTime objects defines the schedule
    workTimes:
      # Each representing a repeating running time period
      - start: 07:00:00
        stop: 18:00:00
        days: [Mon, Tue, Wed, Thu, Fri]
      - start: 08:00:00
        stop: 09:00:00
        days: [Sat, Sun]
```

## Installation

Kubesitter is free to use, you just have to sign up for the [ProfiSea Labs](https://profisealabs.com/) platform and register a cloud account (AWS, Azure, GCP).
After logging in to the platform, select your cloud account in the Account Manager. Press the Details button, go to the API Access tab, and generate an API key with the desired expiration date.

Create a secret in the cluster with the API key and environment ID values:
```sh
kubectl create ns uniskai
kubectl create secret generic uniskai-agent \
  -n uniskai \
  --from-literal=UNISKAI_API_KEY=<API_KEY> \
  --from-literal=UNISKAI_ENV_ID=<ENV_ID>
```

Deploy agent from manifest:
```sh
kubectl apply -f https://uniskai-dev-templates.s3.eu-central-1.amazonaws.com/kubernetes-agent/versions/0.0.x/deployment.yaml
```

## Usage

1. Install a sample application [`emojivoto`](https://github.com/digitalocean/kubernetes-sample-apps/tree/master/emojivoto-example).
2. Create a scheduling policy [`yaml/example-match-labels.yaml`](yaml/example-match-labels.yaml)
```sh
kubectl apply -f yaml/example-match-labels.yaml
```
3. Edit the schedule in the [`yaml/example-match-labels.yaml`](yaml/example-match-labels.yaml) policy to update the desired state and apply policy.

## Metrics

The Kubesitter web server exposes some metrics information you can inspect with `curl`.

```sh
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

The metrics will be scraped by Prometheus if you set up a `ServiceMonitor` for using the chart:
```sh
helm template charts/uniskai-agent --set serviceMonitor.enabled=true | kubectl apply -f -
```

## Running locally

### CRD

Apply the CRD from [cached file](yaml/crds.yaml), or pipe it from `crdgen` to pickup schema changes:

```sh
cargo run -p kubesitter --bin crdgen | kubectl apply -f -
```

### Controller

```sh
UNISKAI_API_KEY="<UNISKAI_API_KEY>" UNISKAI_ENV_ID="<ENV_ID>" cargo r -p agent
```
