[private]
default:
  @just --list --unsorted

image-repository := "profisealabs/uniskai-agent"

build-image tag="latest":
  docker build -t uniskai-agent:{{tag}} .

publish-image tag="latest":
  docker tag uniskai-agent:{{tag}} {{image-repository}}:{{tag}}
  docker push {{image-repository}}:{{tag}}

generate-crds:
  cargo run --bin crdgen -p kubesitter > ./charts/uniskai-agent/crds/kubesitter.yaml
  # combine all CRDs from ./charts/uniskai-agent/crds/ into ./yaml/crds.yaml while separating them with ---
  for file in ./charts/uniskai-agent/crds/*.yaml; do cat "$file"; echo '---'; done > ./yaml/crds.yaml

generate-manifest tag="latest":
  for file in ./charts/uniskai-agent/crds/*.yaml; do cat "$file"; echo '---'; done > ./yaml/deployment.yaml
  helm template charts/uniskai-agent --set version={{tag}} --set image.repository={{image-repository}} >> ./yaml/deployment.yaml

  for file in ./charts/uniskai-agent/crds/*.yaml; do cat "$file"; echo '---'; done > ./yaml/deployment-vpn.yaml
  helm template charts/uniskai-agent --set version={{tag}} --set image.repository={{image-repository}} --set vpnEnabled=true >> ./yaml/deployment-vpn.yaml

publish-manifest tag="latest":
  aws s3 cp --acl public-read ./yaml/deployment.yaml s3://uniskai-dev-templates/kubernetes-agent/versions/{{tag}}/deployment.yaml
  aws s3 cp --acl public-read ./yaml/deployment-vpn.yaml s3://uniskai-dev-templates/kubernetes-agent/versions/{{tag}}/deployment-vpn.yaml

release tag="latest":
  just build-image {{tag}}
  just publish-image {{tag}}
  just generate-manifest {{tag}}
  just publish-manifest {{tag}}

publish-private-azure tag="latest":
  az login
  az acr login --name uniskaidevoa
  docker tag uniskai-agent:{{tag}} uniskaidevoa.azurecr.io/uniskai-agent:{{tag}}
  docker push uniskaidevoa.azurecr.io/uniskai-agent:{{tag}}

  helm template charts/uniskai-agent --set version={{tag}} --set image.repository=uniskaidevoa.azurecr.io/uniskai-agent > ./yaml/deployment-azure.yaml
  helm template charts/uniskai-agent --set version={{tag}} --set image.repository=uniskaidevoa.azurecr.io/uniskai-agent --set vpnEnabled=true > ./yaml/deployment-azure-vpn.yaml

  aws s3 cp --acl public-read ./yaml/deployment-azure.yaml s3://uniskai-dev-templates/kubernetes-agent/versions/{{tag}}/deployment-azure.yaml
  aws s3 cp --acl public-read ./yaml/deployment-azure-vpn.yaml s3://uniskai-dev-templates/kubernetes-agent/versions/{{tag}}/deployment-azure-vpn.yaml

publish-private-aws tag="latest":
  aws ecr get-login-password --region eu-west-1 | docker login --username AWS --password-stdin 199042988758.dkr.ecr.eu-west-1.amazonaws.com
  docker tag uniskai-agent:{{tag}} 199042988758.dkr.ecr.eu-west-1.amazonaws.com/uniskai-agent-dev:{{tag}}
  docker push 199042988758.dkr.ecr.eu-west-1.amazonaws.com/uniskai-agent-dev:{{tag}}

  helm template charts/uniskai-agent --set version={{tag}} --set image.repository=199042988758.dkr.ecr.eu-west-1.amazonaws.com/uniskai-agent-dev > ./yaml/deployment-aws.yaml
  helm template charts/uniskai-agent --set version={{tag}} --set image.repository=199042988758.dkr.ecr.eu-west-1.amazonaws.com/uniskai-agent-dev --set vpnEnabled=true > ./yaml/deployment-aws-vpn.yaml

  aws s3 cp --acl public-read ./yaml/deployment-aws.yaml s3://uniskai-dev-templates/kubernetes-agent/versions/{{tag}}/deployment-aws.yaml
  aws s3 cp --acl public-read ./yaml/deployment-aws-vpn.yaml s3://uniskai-dev-templates/kubernetes-agent/versions/{{tag}}/deployment-aws-vpn.yaml

release-dev tag="latest":
  just build-image {{tag}}
  just publish-private-azure {{tag}}
  just publish-private-aws {{tag}}
  just generate-crd
  just publish-crd

# install crd into the cluster
install-crd: generate
  kubectl apply -f yaml/crd.yaml
  
publish-crd:
  aws s3 cp --acl public-read ./yaml/crd.yaml s3://uniskai-dev-templates/kubernetes-agent/crd.yaml

generate:
  cargo run --bin crdgen -p kubesitter > ./yaml/crd.yaml
  helm template charts/uniskai-agent > yaml/deployment.yaml

# run with opentelemetry
run-telemetry:
  OPENTELEMETRY_ENDPOINT_URL=http://127.0.0.1:55680 RUST_LOG=info,kube=trace,controller=debug cargo run --features=telemetry

# run without opentelemetry
run:
  RUST_LOG=info,kube=debug,controller=debug cargo run

# format with nightly rustfmt
fmt:
  cargo +nightly fmt

# run unit tests
test-unit:
  cargo test
# run integration tests
test-integration: install-crd
  cargo test -- --ignored
# run telemetry tests
test-telemetry:
  OPENTELEMETRY_ENDPOINT_URL=http://127.0.0.1:55680 cargo test --lib --all-features -- get_trace_id_returns_valid_traces --ignored

# compile for musl (for docker image)
compile features="":
  #!/usr/bin/env bash
  docker run --rm \
    -v cargo-cache:/root/.cargo \
    -v $PWD:/volume \
    -w /volume \
    -t clux/muslrust:stable \
    cargo build --release --features={{features}} --bin controller
  cp target/x86_64-unknown-linux-musl/release/controller .

[private]
_build features="":
  just compile {{features}}
  docker build -t clux/controller:local .

# docker build base
build-base: (_build "")
# docker build with telemetry
build-otel: (_build "telemetry")


# local helper for test-telemetry and run-telemetry
# forward grpc otel port from svc/promstack-tempo in monitoring
forward-tempo:
  kubectl port-forward -n monitoring svc/promstack-tempo 55680:4317
