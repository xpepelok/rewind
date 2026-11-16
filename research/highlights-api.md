# API «Лучших моментов» NVIDIA (GfeSDK)

Как игра сообщает оверлею NVIDIA о важных моментах. Разобрано по библиотеке
`GfeSDK.dll`, которую поставляет Fortnite
(`FortniteGame\Binaries\ThirdParty\NVIDIA\Win64\GfeSDK.dll`), и по серверной
части в `NVIDIA app\CEF\plugins\NVIDIA Overlay\ShareServer.dll`.
Полное описание сообщений лежит рядом: [gfesdk.proto.txt](gfesdk.proto.txt).

## Что проверено, а что нет

| Что | Статус |
|---|---|
| Состав сообщений и номера полей | достоверно: извлечено из описания, встроенного в `GfeSDK.dll` |
| Транспорт — канал сообщений NVIDIA (`MessageBus.dll`) | достоверно: в библиотеке есть класс `MessageBusIPC` и чтение `InstallPath` |
| Имена участников канала | **не проверено**: в обеих библиотеках рядом лежат строки `GfeSDKCore`, `GfeSDK`, `NvNodeSDKAPI`, но какая пара — клиент, а какая — сервер, неизвестно |
| Тип сообщения канала (`generic.type`) | **не проверено** |
| Нужен ли ответу `reference_uniqueid` запроса | **не проверено**, Rewind его проставляет |
| Значения кодов разрешений и ошибок | по памяти из открытого SDK, **не сверено** |
| Работа с настоящей игрой | **не проверялась ни разу** |

Rewind пока подключается под четырьмя вероятными именами сразу и отвечает
тому, кто написал.

## Как это устроено

1. Разработчик встраивает в игру `GfeSDK.dll`.
2. При запуске игра вызывает `NVGSDK_Create` — библиотека подключается к каналу
   сообщений NVIDIA и шлёт `CreateSDKInstanceRequest`.
3. Игра описывает свои виды моментов (`ConfigureHighlightsRequest`).
4. Во время матча игра открывает группу и шлёт команды «сохрани видео» или
   «сделай снимок».
5. В конце матча игра может попросить показать сводку (`OpenGroupSummaryRequest`).

Оверлей сам ничего в игре не отслеживает: все события приходят от игры.

## Функции библиотеки (то, что вызывает игра)

| Функция | Сообщение |
|---|---|
| `NVGSDK_Create` | `CreateSDKInstanceRequest` |
| `NVGSDK_Release` | `DestroySDKInstanceRequest` |
| `NVGSDK_RequestPermissionsAsync` | `RequestPermissionRequest` |
| `NVGSDK_GetUILanguageAsync` | `GetUILanguageRequest` |
| `NVGSDK_Highlights_ConfigureAsync` | `ConfigureHighlightsRequest` |
| `NVGSDK_Highlights_GetUserSettingsAsync` | `GetUserConfigRequest` |
| `NVGSDK_Highlights_OpenGroupAsync` | `OpenGroupRequest` |
| `NVGSDK_Highlights_CloseGroupAsync` | `CloseGroupRequest` |
| `NVGSDK_Highlights_SetVideoHighlightAsync` | `SetVideoHighlightRequest` |
| `NVGSDK_Highlights_SetScreenshotHighlightAsync` | `SetScreenshotHighlightRequest` |
| `NVGSDK_Highlights_GetNumberOfHighlightsAsync` | `GetNumberOfHighlightsInGroupRequest` |
| `NVGSDK_Highlights_OpenSummaryAsync` | `OpenGroupSummaryRequest` |
| `NVGSDK_Poll` | — (разбор очереди ответов внутри библиотеки) |

## Конверт

Все сообщения — protobuf (proto2), завёрнутые в `SDKIPCMessage`:

```proto
message SDKIPCMessage {
  optional SDKIPCRequest      request      = 1;
  optional SDKIPCResponse     response     = 2;
  optional SDKIPCNotification notification = 3;
}

message SDKIPCRequest {
  required int32 version_major = 1;
  required int32 version_minor = 2;
  optional int32 version_build = 3;
  optional string version_hash = 4;
  required SDKComponent component = 5;   // CORE=0 SHARE=1 INSTALLER=2 OPS=3
  // далее ровно один запрос, поля 10–25
}

message SDKIPCResponse {
  required int32 retval    = 1;          // 0 — успех
  optional string retstatus = 2;
  // далее необязательный ответ, поля 10–14
}
```

## Запросы игры

| Поле | Запрос | Содержимое | Ответ |
|---|---|---|---|
| 19 | `CreateSDKInstanceRequest` | `client`, `clientPID` (2), `gameName` (4), `clientGfeVersion` (5), `scopes` (6), `clientProcessPath` (7) | `CreateSDKInstanceResponse` (11) |
| 20 | `DestroySDKInstanceRequest` | — | только `retval` |
| 21 | `IsClientAliveRequest` | — | `IsClientAliveResponse` (12) |
| 10 | `GetUILanguageRequest` | — | `GetUILanguageResponse` (10) |
| 24 | `RequestPermissionRequest` | `scopes` (1) | только `retval` |
| 16 | `ConfigureHighlightsRequest` | `gameArt` (2), `highlightTable` (3), `defaultLocale` (4) | только `retval` |
| 23 | `GetUserConfigRequest` | — | `GetUserConfigResponse` (14) |
| 11 | `OpenGroupRequest` | `groupId` (1), названия по языкам (2) | только `retval` |
| 12 | `CloseGroupRequest` | `groupId` (1), `destroyHighlights` (2) | только `retval` |
| 14 | `SetVideoHighlightRequest` | `groupId` (2), `highlightId` (3), `startDelta` (4), `endDelta` (5), `baseTimeStamp` (6) | только `retval` |
| 13 | `SetScreenshotHighlightRequest` | `groupId` (2), `highlightId` (3) | только `retval` |
| 22 | `GetNumberOfHighlightsInGroupRequest` | `groupId` (1), фильтры (2, 3) | `GetNumberOfHighlightsInGroupResponse` (13) |
| 17 | `OpenGroupSummaryRequest` | `groupTable` (2): `groupId`, фильтры | только `retval` |
| 15 | `ToggleOverlayRequest` | `visibilityType`: OPEN=1 CLOSE=2 TOGGLE=3 OPEN_PREFS=4 | только `retval` |
| 25 | `OpsOptimizeRequest` | путь к exe | не поддерживается |

### Вид момента

```proto
message HighlightDefinition {
  required string highlightId = 1;       // например "kill", "victory"
  optional bool   userInterest = 3;      // включён ли по умолчанию
  optional int32  highlightTags = 4;
  optional int32  significance = 5;
  optional bytes  icon = 6;
  repeated LocalePair highlightLocaleNamePairs = 7;   // {"ru-RU", "Убийство"}
}
```

### Видеомомент

`startDelta` и `endDelta` — миллисекунды относительно «сейчас».
`startDelta` обычно отрицательный (сколько взять до события), `endDelta` может
быть положительным (сколько дописать после). Пример: `-5000 … +2000` — пять
секунд до и две после.

## Ответы оверлея

```proto
message CreateSDKInstanceResponse {
  optional string installedGfeVersion = 1;
  optional int32  installedGfeSDKIPCVersionMajor = 2;
  optional int32  installedGfeSDKIPCVersionMinor = 3;
  repeated ScopePermission scopePermission = 4;     // {scope, permission}
}

message GetUserConfigResponse {
  required bool gameEnabled = 1;
  repeated HighlightConfig highlightTable = 2;      // {highlightId, userEnabled}
}

message GetUILanguageResponse { required string localeName = 1; }   // "ru-RU"
message IsClientAliveResponse { required bool alive = 1; }
message GetNumberOfHighlightsInGroupResponse { required int32 numberOfHighlights = 1; }
```

Области (`scope`), по памяти: 0 — моменты, 1 — запись видео, 2 — снимки,
3 — оптимизация. Разрешение (`permission`): 0 — выдано, 1 — отказано,
2 — спросить, 3 — неизвестно.

## Уведомления от оверлея к игре

```proto
message SDKIPCNotification {
  required SDKComponent component = 1;
  optional PermissionsChangedNotification permissionsChangedNotification = 10;
  optional OverlayStateNotification overlayStateNotification = 11;   // {open, state}
}
```

Rewind их пока не отправляет.

## Что делает Rewind

Код: `crates/nvbus/src/highlights.rs` (сообщения), `desk.rs` (логика ответов),
`moments.rs` (поток, который слушает канал).

| Запрос игры | Действие |
|---|---|
| `Create` | выдать все запрошенные области, вернуть ту же версию протокола, запомнить игру |
| `Configure` | сохранить виды моментов в настройки, выбор пользователя не трогать |
| `GetUserConfig` | вернуть, какие виды пользователь оставил включёнными |
| `SetVideoHighlight` | если вид включён — сохранить отрезок из мгновенного повтора |
| `SetScreenshotHighlight` | если вид включён — сделать снимок экрана |
| `OpenGroupSummary`, `ToggleOverlay` | открыть галерею в оверлее |
| `GetNumberOfHighlights` | вернуть счётчик моментов группы |
| остальное | ответить «успех» |

Для видеомоментов должен быть включён мгновенный повтор: отрезок вырезается
из его буфера.

## Где это работает

Только в играх, в которые разработчик встроил `GfeSDK.dll`. Из установленных
на этом компьютере — в Fortnite. Fortnite защищён античитом Easy Anti-Cheat;
Rewind в игру не внедряется и лишь отвечает на сообщения, которые игра сама
шлёт в канал NVIDIA, но как античит относится к стороннему ответчику, не
проверено.
