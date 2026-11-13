# UtaLink

**`UtaLink.exe`를 더블클릭하면 실행됩니다.** 별도 실행 스크립트나 Python 설치는 필요 없습니다.

곡을 검색하고 한글 발음 또는 노래방 반주 영상을 선택해 링크를 복사하거나 VRChat 중계를 시작할 수 있습니다.

## 폴더 구성

| 항목 | 용도 |
| --- | --- |
| `UtaLink.exe` | Windows 앱 |
| `config.txt` | 중계 설정. 개인 토큰이 있으므로 공유하지 마세요. |
| `window-state.txt` | 앱이 자동 저장하는 창 크기 |
| `tools/` | 중계에 필요한 실행 도구 |
| `assets/` | 아이콘, 내장 글꼴, 원본 이미지 |
| `docs/` | 사용 설명과 디자인 검토 자료 |
| `src/`, `web/` | 앱과 웹 소스 코드 |

`Cargo.toml`, `Cargo.lock`, `build.rs`는 소스 빌드에 사용합니다. `Dockerfile`과 `render.yaml`은 웹 배포 설정입니다.

[자세한 사용법과 빌드 방법](docs/manual.md) / [검색 판정 규칙](docs/search-policy.md) / [디자인 검토 기록](docs/design/README.md)
