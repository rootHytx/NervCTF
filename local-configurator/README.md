# NervCTF Local Configurator

The Local Configurator manages CTF challenge deployments and configuration for the NervCTF platform.

## Features

- **Challenge Management**: Recursively process CTF challenges using `ctfcli`
- **Container Orchestration**: Automatically deploy and manage the remote-monitor container
- **Configuration Sync**: Send challenge configurations to remote components

## Challenge Manager

The challenge manager provides recursive operations on CTF challenges:

### Installation
```bash
pip3 install ctfcli
```

### Usage
```bash
./local-configurator challenges [OPERATION] --path [PATH]
```

### Operations
| Command   | Description                          |
|-----------|--------------------------------------|
| `sync`    | Sync challenge with CTFd             |
| `install` | Install challenge dependencies       |
| `lint`    | Lint challenge configuration         |
| `verify`  | Verify challenge setup               |
| `deploy`  | Deploy challenge                     |
| `push`    | Push challenge to CTFd               |

### Options
| Option          | Description                          | Default           |
|-----------------|--------------------------------------|-------------------|
| `--path <PATH>` | Path to challenges directory         | `./challenges`    |

### Example
```bash
./local-configurator challenges sync --path ./my-challenges
```

## Docker Build
```bash
docker build -t local-configurator .
```

## Environment Variables
| Variable         | Description                          |
|------------------|--------------------------------------|
| `DOCKER_HOST`    | Docker daemon host address           |
| `CTFD_URL`       | CTFd instance URL                    |
| `CTFD_API_KEY`   | CTFd admin API key                   |

## License
MIT