# Example: Client → RetroTLS → HTTPS API

이 예제는 RetroTLS를 통해 로컬 HTTP 클라이언트가 외부 HTTPS API에 접근하는 흐름을 보여줍니다.

## 아키텍처

```
Client (curl) → RetroTLS (127.0.0.1:8080) → https://httpbin.org
```

## 실행 방법

### 1. RetroTLS 설정

```bash
cat > config.yaml << 'EOF'
access_log: true
listeners:
  - listen: "127.0.0.1:8080"
    upstream: "https://httpbin.org"
EOF
```

### 2. RetroTLS 실행 (터미널 1)

```bash
cd ..
./target/release/retrotls --config example/config.yaml
```

### 3. 테스트 실행 (터미널 2)

```bash
# GET 요청 테스트
curl http://127.0.0.1:8080/get

# POST 요청 테스트
curl -X POST http://127.0.0.1:8080/post \
  -H "Content-Type: application/json" \
  -d '{"test": "hello"}'

# 헤더 확인
curl http://127.0.0.1:8080/headers
```

## 기대 결과

RetroTLS 로그에서 다음과 같은 access log가 출력됩니다:

```
INFO 127.0.0.1:xxxxx -> 127.0.0.1:8080 GET /get 200 xxms
INFO 127.0.0.1:xxxxx -> 127.0.0.1:8080 POST /post 200 xxms
```

## 테스트 스크립트

자동화된 테스트는 `test.sh`를 실행하세요:

```bash
chmod +x test.sh
./test.sh
```
