1. Get your ovpn and auth creds
```
kubectl create secret generic vpn-config --from-file=client.ovpn -n uniskai
kubectl create secret generic vpn-auth --from-file=auth.txt -n uniskai
```

2. Install routeconfig
```
kubectl apply -f .\yaml\vpn\routeconfig.yaml -n uniskai
```