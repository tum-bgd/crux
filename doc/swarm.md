# Docker Swarm

Create a Docker Swarm with a manager and several nodes.

Then checkout the repository on the manager and distribute the data folder onto each node.

<!-- Run swarm dashboard (optional).

```bash
docker stack deploy -c docker-compose.dashboard.yml sd
``` -->

Setup a registry to distribute docker image.

```bash
docker service create --name registry --constraint node.role==manager --publish published=5000,target=5000 registry:2
```

Build the docker image and push to the registry.

```bash
docker build . -t localhost:5000/crux
docker push localhost:5000/crux
```

Start coordinater service on the manager node.

```bash
docker service create --name coordinator --constraint node.role==manager --env PORT=3000 --env RUST_LOG=crux_server=debug,tower_http=debug --mount type=tmpfs,destination=/dev/shm,tmpfs-size=34359738368 --network host localhost:5000/crux
```

Create a service with some workers (may need to adapt the COORDINATORS variable to point to the manager).

```bash
docker service create --name crux --replicas 8 --constraint node.role!=manager --mount type=bind,source=/home/setup/crux/data,destination=/crux/data --env COORDINATORS=http://10.157.144.36:3000/workers --env RUST_LOG=crux_server=debug,tower_http=debug --mount type=tmpfs,destination=/dev/shm,tmpfs-size=34359738368 --network host localhost:5000/crux
```

