# Lumiere — Sprint Overview

## Sprint Map

| # | Sprint | Status | Dependencies | Crates Affected |
|---|--------|--------|-------------|-----------------|
| 01 | [Foundation & Infrastructure](sprint-01-foundation.md) | Not Started | — | workspace, lumiere-server, lumiere-models |
| 02 | [Database Schemas & Snowflake ID](sprint-02-database-schemas.md) | Not Started | Sprint 01 | lumiere-models, lumiere-db |
| 03 | [Authentication](sprint-03-authentication.md) | Not Started | Sprint 02 | lumiere-auth, lumiere-server |
| 04 | [User System](sprint-04-user-system.md) | Not Started | Sprint 03 | lumiere-server, lumiere-db |
| 05 | [Server System](sprint-05-server-system.md) | Not Started | Sprint 03 | lumiere-server, lumiere-db |
| 06 | [Channel System](sprint-06-channel-system.md) | Not Started | Sprint 05 | lumiere-server, lumiere-db |
| 07 | [Permission System](sprint-07-permissions.md) | Not Started | Sprint 05, 06 | lumiere-permissions, lumiere-server |
| 08 | [WebSocket Gateway](sprint-08-websocket-gateway.md) | Not Started | Sprint 03, 07 | lumiere-gateway, lumiere-nats, lumiere-server |
| 09 | [Messaging Core](sprint-09-messaging-core.md) | Not Started | Sprint 08 | lumiere-server, lumiere-db, lumiere-gateway |
| 10 | [Messaging Advanced](sprint-10-messaging-advanced.md) | Not Started | Sprint 09, 12 | lumiere-server, lumiere-db, lumiere-media |
| 11 | [Typing & Read States](sprint-11-typing-read-states.md) | Not Started | Sprint 09 | lumiere-gateway, lumiere-db |
| 12 | [File & Media System](sprint-12-file-media.md) | Not Started | Sprint 01 | lumiere-media, lumiere-server |
| 13 | [Search System](sprint-13-search.md) | Not Started | Sprint 09 | lumiere-search, lumiere-server |
| 14 | [Voice & Video](sprint-14-voice-video.md) | Not Started | Sprint 08 | lumiere-voice, lumiere-server, lumiere-gateway |
| 15 | [Push Notifications](sprint-15-push-notifications.md) | Not Started | Sprint 09 | lumiere-push, lumiere-server |
| 16 | [Data Services Layer](sprint-16-data-services.md) | Not Started | Sprint 09 | lumiere-data-services |
| 17 | [Rate Limiting & Security](sprint-17-rate-limiting-security.md) | Not Started | Sprint 08 | lumiere-server, lumiere-gateway |
| 18 | [Moderation System](sprint-18-moderation.md) | Not Started | Sprint 07, 09 | lumiere-server, lumiere-db |
| 19 | [Bot & Integration Framework](sprint-19-bot-framework.md) | Not Started | Sprint 09, 07 | lumiere-server, lumiere-db |
| 20 | [E2E Encryption](sprint-20-e2e-encryption.md) | Not Started | Sprint 09 | lumiere-server, lumiere-db, lumiere-gateway |
| 21 | [Monitoring & Observability](sprint-21-monitoring.md) | Not Started | Sprint 01 | all crates |
| 22 | [Performance & Load Testing](sprint-22-performance.md) | Not Started | Sprint 16 | all crates |
| 23 | [Deployment & DevOps](sprint-23-deployment.md) | Not Started | Sprint 21 | infrastructure |

## Dependency Graph

```
Sprint 01 (Foundation)
    |
    v
Sprint 02 (DB Schemas + Snowflake ID)
    |
    v
Sprint 03 (Authentication)
    |
    +----------+----------+
    |          |          |
    v          v          v
Sprint 04  Sprint 05  Sprint 12
(Users)    (Servers)  (Files)
               |
               v
           Sprint 06
           (Channels)
               |
               v
           Sprint 07
           (Permissions)
               |
               v
           Sprint 08
           (WebSocket Gateway)
               |
    +----------+----------+----------+
    |          |          |          |
    v          v          v          v
Sprint 09  Sprint 14  Sprint 17  Sprint 11
(Messaging (Voice)    (Security) (Typing)
 Core)
    |
    +-----+-----+-----+-----+
    |     |     |     |     |
    v     v     v     v     v
  S10   S13   S15   S16   S18   S19   S20
  (Adv) (Srch) (Push) (Data) (Mod) (Bot) (E2E)
                              |
                              v
                           Sprint 22
                           (Performance)
                              |
                              v
                           Sprint 21 --> Sprint 23
                           (Monitor)    (Deploy)
```

## How to Use This Plan

1. Each sprint has its own detailed document in this directory
2. Sprints should be completed in order (respecting dependencies)
3. Each sprint document contains: goals, tasks, API endpoints, data models, acceptance criteria
4. When starting a sprint, update its status in this file to "In Progress"
5. When completing a sprint, update its status to "Completed" with the date
