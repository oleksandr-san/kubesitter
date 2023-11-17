1. Add your ovpn and auth creds
```
kubectl create secret generic vpn-config --from-file=client.ovpn -n uniskai
kubectl create secret generic vpn-auth --from-file=auth.txt -n uniskai
```

auth.txt should be in the format:
```
username
password
```
