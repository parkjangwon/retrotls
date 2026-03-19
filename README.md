# RetroTLS

<img width="1024" height="559" alt="image" src="https://github.com/user-attachments/assets/a2b1a87e-cb14-482e-bfd8-2b2c43d98577" />

구형 HTTP 클라이언트를 현대 HTTPS API에 연결하는 초경량 브릿지 프록시

## 개요

RetroTLS는 레거시 HTTP 클라이언트가 현대적인 HTTPS API에 연결할 수 있도록 돕는 최소한의 고성능 단일 바이너리 브릿지 프록시입니다. 평문 HTTP 요청을 수신하여 HTTPS 업스트림 서버로 전달합니다.

> **영문 문서**: [README.en.md](README.en.md)

### 핵심 특징

- **초경량**: 단일 목적, 단일 바이너리 (~2.3MB)
- **고성능**: Tokio 기반 비동기 I/O, 스트리밍 바디, 커넥션 풀링
- **보안**: TLS 1.2+ 전용, 인증서 검증
- **단순함**: YAML 설정, 웹 UI 없음, 복잡한 기능 없음

## 설치

### 원라인 인스톨 (권장)

```bash
curl -fsSL https://raw.githubusercontent.com/parkjangwon/retrotls/main/install.sh | sh
```

또는

```bash
wget -qO- https://raw.githubusercontent.com/parkjangwon/retrotls/main/install.sh | sh
```

설치 후 `~/.local/bin`이 PATH에 없다면 다음을 추가하세요:
```bash
export PATH="$HOME/.local/bin:$PATH"
```

### 업데이트

인스톨과 동일합니다. 최신 버전으로 자동 업데이트됩니다:
```bash
curl -fsSL https://raw.githubusercontent.com/parkjangwon/retrotls/main/install.sh | sh
```

### 제거

```bash
curl -fsSL https://raw.githubusercontent.com/parkjangwon/retrotls/main/install.sh | sh -s -- --uninstall
```

### 수동 설치

릴리즈 페이지에서 OS에 맞는 바이너리를 다운로드하세요:  
https://github.com/parkjangwon/retrotls/releases

### 소스에서 빌드

```bash
git clone https://github.com/retrotls/retrotls
cd retrotls
cargo build --release
```

## 사용법

### 빠른 시작

RetroTLS 실행:
```bash
retrotls
```

최초 실행 시 `~/.config/retrotls/config.yaml`이 자동 생성됩니다.  
설정 파일을 수정한 후 다시 실행하세요.

또는 직접 설정 파일 생성:
```bash
retrotls
```

### CLI 옵션

```
retrotls [OPTIONS]

Options:
  -c, --config <FILE>    설정 파일 경로
      --check            설정 검증 후 종료
      --version          버전 출력
      --log-level <LEVEL> 로그 레벨 (debug, info, warn, error)
  -h, --help             도움말 출력
```

### 요청 흐름 예시

클이언트 요청:
```bash
curl http://127.0.0.1:8080/users?id=1
```

업스트림으로 전달:
```
https://api.example.com/users?id=1
```

## 설정

설정 파일 경로: `~/.config/retrotls/config.yaml`

### 전체 예시

```yaml
access_log: true

listeners:
  - listen: "127.0.0.1:8080"
    upstream: "https://api1.com"
  
  - listen: "127.0.0.1:8081"
    upstream: "https://api2.com/base"
```

### 설정 옵션

#### Listeners

- `listen`: 수신 대기 소켓 주소 (예: "127.0.0.1:8080")
- `upstream`: 요청을 전달할 HTTPS URL ("https://"로 시작해야 함)

경로 처리 예시:
- 클라이언트: `/v1/test` → 업스트림: `https://api.com/base/v1/test`
- 클라이언트: `/` → 업스트림: `https://api.com/base/`

#### Logging

- `access_log`: 접근 로깅 활성화 (기본값: true)

접근 로그 형식:
```
<timestamp> <client_addr> -> <bind_addr> <method> <path> <status> <latency_ms>ms
```

## 테스트 예제

`example/` 디렉토리에서 RetroTLS 테스트를 합니다:

```bash
cd example

# RetroTLS 실행 (터미널 1)
../target/release/retrotls --config config.yaml

# 테스트 실행 (터미널 2)
./test.sh

# 또는 수동 테스트
curl http://127.0.0.1:8080/get
curl -X POST http://127.0.0.1:8080/post -H "Content-Type: application/json" -d '{"test": "hello"}'
```

## Systemd 서비스

사용자 systemd 서비스 파일이 제공됩니다 (`retrotls.service`):

```bash
cp retrotls.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable retrotls
systemctl --user start retrotls
systemctl --user status retrotls
journalctl --user -u retrotls -f
```

## 빌드

### 개발 빌드
```bash
cargo build
```

### 릴리즈 빌드 (최적화)
```bash
cargo build --release
```

### 테스트 실행
```bash
cargo test
```

## 지원 기능

- HTTP/1.1 요청 전달
- TLS 1.2/1.3 HTTPS 업스트림
- 요청/응답 스트리밍
- 커넥션 풀링 및 keep-alive
- Hop-by-hop 헤더 필터링
- X-Forwarded-* 헤더 추가
- Graceful shutdown (SIGINT/SIGTERM)

## 아키텍처

RetroTLS는 최소한의 아키텍처를 따릅니다:

1. **HTTP Listener**: 설정된 주소에 바인딩
2. **Request Handler**: HTTP 요청을 HTTPS 업스트림으로 전달
3. **TLS Client**: 업스트림과의 보안 연결 설정
4. **Response Streamer**: 업스트림 응답을 클라이언트에 반환

## 보안 고려사항

- 항상 TLS 1.2 이상 사용
- 프로덕션에서는 인증서 검증 활성화 유지
- 특정 요구사항이 없는 경우 localhost(127.0.0.1)에 바인딩
- root 권한 없이 실행
- 기본적으로 민감한 데이터는 로그에 남지 않음

## 라이선스

MIT License - 자세한 내용은 LICENSE 파일을 참조하세요.

## 문제 해결

### "Failed to load config"
설정 파일이 `~/.config/retrotls/config.yaml`에 존재하는지 확인하거나 `--config`로 경로를 지정하세요.

### "Bind failed"
포트가 이미 사용 중이 아닌지, 바인딩 권한이 있는지 확인하세요.

### "Upstream connection failed"
업스트림 URL이 올바르고 RetroTLS 호스트에서 접근 가능한지 확인하세요.

### "Gateway Timeout"
업스트림이 느린 경우 설정에서 `request_ms`를 증가시키세요.

---

**RetroTLS** - 구형 클라이언트와 현대 API를 잇는 작고 단단한 다리
