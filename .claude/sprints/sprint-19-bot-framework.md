# Sprint 19 — Bot & Integration Framework

**Status:** Not Started
**Dependencies:** Sprint 09, Sprint 07
**Crates:** lumiere-server, lumiere-db

## Goal

Bot account system, OAuth2 authorization, webhook system, interaction framework (slash commands, buttons, select menus), and bot API with rate limiting.

## Tasks

### 19.1 — Bot Accounts

Bot accounts are users with `is_bot = true`:

```
POST /api/v1/applications
    Body: { name, description? }
    Response: { id, name, description, bot: { id, token } }
    Auth: Authenticated user (becomes bot owner)

GET /api/v1/applications/@me
    Response: [Application objects]

PATCH /api/v1/applications/:app_id
    Body: { name?, description?, icon? }

POST /api/v1/applications/:app_id/bot/reset-token
    Response: { token: "new_bot_token" }
```

Bot token format: `Bot {base64(bot_id)}:{random_secret}`

Bot authentication:
```
Authorization: Bot <token>
```

Bots use the same API endpoints as regular users but with some restrictions:
- Cannot add friends
- Cannot join servers on their own (must be invited via OAuth2)
- Higher rate limits on messaging (for webhook-like behavior)
- Cannot use voice (unless explicitly designed for it)

### 19.2 — OAuth2

```
GET /api/v1/oauth2/authorize
    Query: ?client_id=X&scope=bot&permissions=8&redirect_uri=Y&response_type=code
    Response: Redirect to consent page (handled by frontend)

POST /api/v1/oauth2/token
    Body: { grant_type: "authorization_code", code: "...", redirect_uri: "..." }
    Response: { access_token, token_type: "Bearer", expires_in, refresh_token, scope }

POST /api/v1/oauth2/token (refresh)
    Body: { grant_type: "refresh_token", refresh_token: "..." }
```

OAuth2 scopes:
```rust
pub enum OAuthScope {
    Bot,                // Add bot to server
    Identify,           // Access user ID and username
    Email,              // Access user email
    Guilds,             // Access user's server list
    GuildsJoin,         // Add user to a server
    MessagesRead,       // Read messages in channels bot has access to
    Webhook,            // Create webhooks
}
```

Bot invite flow:
1. User clicks invite link with OAuth2 authorize URL
2. User selects which server to add bot to
3. User approves permissions
4. Bot is added as member with specified permissions
5. GUILD_CREATE event dispatched to bot's gateway connection

### 19.3 — Webhook System

**Incoming Webhooks** (send messages to a channel via URL):

```
POST /api/v1/channels/:channel_id/webhooks
    Body: { name, avatar? }
    Auth: MANAGE_WEBHOOKS
    Response: { id, token, channel_id, name, avatar, url }
    URL format: /api/v1/webhooks/{id}/{token}

GET /api/v1/channels/:channel_id/webhooks
    Auth: MANAGE_WEBHOOKS
    Response: [Webhook objects]

GET /api/v1/servers/:server_id/webhooks
    Auth: MANAGE_WEBHOOKS

PATCH /api/v1/webhooks/:webhook_id
    Body: { name?, avatar?, channel_id? }
    Auth: MANAGE_WEBHOOKS

DELETE /api/v1/webhooks/:webhook_id
    Auth: MANAGE_WEBHOOKS
```

Execute webhook (no auth required — token in URL):
```
POST /api/v1/webhooks/:webhook_id/:token
    Body: {
        content?: "Hello from webhook",
        username?: "Custom Name",      // Override webhook name
        avatar_url?: "...",            // Override webhook avatar
        embeds?: [...],
        tts?: false,
    }
    Response: Message object (if ?wait=true) or 204
```

### 19.4 — Interaction Framework

**Slash Commands:**

Register commands:
```
POST /api/v1/applications/:app_id/commands
    Body: {
        name: "ping",
        description: "Check bot latency",
        options: [
            {
                name: "format",
                description: "Response format",
                type: 3,  // STRING
                required: false,
                choices: [
                    { name: "Simple", value: "simple" },
                    { name: "Detailed", value: "detailed" },
                ]
            }
        ]
    }

// Server-specific commands:
POST /api/v1/applications/:app_id/servers/:server_id/commands
```

Command option types:
```rust
pub enum CommandOptionType {
    SubCommand = 1,
    SubCommandGroup = 2,
    String = 3,
    Integer = 4,
    Boolean = 5,
    User = 6,
    Channel = 7,
    Role = 8,
    Mentionable = 9,   // User or Role
    Number = 10,       // Float
    Attachment = 11,
}
```

**Interaction Delivery:**

When a user invokes a slash command:
1. Server creates an Interaction
2. Sends to bot via gateway (INTERACTION_CREATE event) or webhook (if configured)
3. Bot responds within 3 seconds

```
POST /api/v1/interactions/:interaction_id/:token/callback
    Body: {
        type: 4,  // CHANNEL_MESSAGE_WITH_SOURCE
        data: {
            content: "Pong! Latency: 42ms",
            embeds: [...],
            flags: 64,  // EPHEMERAL (only visible to invoker)
        }
    }
```

Interaction response types:
```rust
pub enum InteractionResponseType {
    Pong = 1,
    ChannelMessageWithSource = 4,
    DeferredChannelMessageWithSource = 5,
    DeferredUpdateMessage = 6,
    UpdateMessage = 7,
    AutocompleteResult = 8,
    Modal = 9,
}
```

### 19.5 — Message Components

Buttons and select menus in messages:

```json
{
    "content": "Choose an option:",
    "components": [
        {
            "type": 1,  // ActionRow
            "components": [
                {
                    "type": 2,  // Button
                    "style": 1, // Primary
                    "label": "Click me",
                    "custom_id": "button_1"
                },
                {
                    "type": 3,  // StringSelect
                    "custom_id": "select_1",
                    "options": [
                        { "label": "Option 1", "value": "opt1" },
                        { "label": "Option 2", "value": "opt2" }
                    ]
                }
            ]
        }
    ]
}
```

Component types:
```rust
pub enum ComponentType {
    ActionRow = 1,
    Button = 2,
    StringSelect = 3,
    TextInput = 4,     // For modals
    UserSelect = 5,
    RoleSelect = 6,
    MentionableSelect = 7,
    ChannelSelect = 8,
}
```

When a user clicks a button or selects an option → INTERACTION_CREATE event to bot.

### 19.6 — Bot Rate Limits

Bots have separate, higher rate limits:
- Message send: 5 per second per channel
- Global: 50 requests per second
- Webhook execute: 5 per second per webhook
- Bulk message operations: dedicated limits

### 19.7 — Bot Gateway Intents

Bots specify which events they want to receive:

```rust
bitflags::bitflags! {
    pub struct GatewayIntents: u32 {
        const GUILDS                  = 1 << 0;
        const GUILD_MEMBERS           = 1 << 1;  // Privileged
        const GUILD_MODERATION        = 1 << 2;
        const GUILD_EMOJIS            = 1 << 3;
        const GUILD_INTEGRATIONS      = 1 << 4;
        const GUILD_WEBHOOKS          = 1 << 5;
        const GUILD_INVITES           = 1 << 6;
        const GUILD_VOICE_STATES      = 1 << 7;
        const GUILD_PRESENCES         = 1 << 8;  // Privileged
        const GUILD_MESSAGES          = 1 << 9;
        const GUILD_MESSAGE_REACTIONS = 1 << 10;
        const GUILD_MESSAGE_TYPING    = 1 << 11;
        const DIRECT_MESSAGES         = 1 << 12;
        const DIRECT_MESSAGE_REACTIONS = 1 << 13;
        const DIRECT_MESSAGE_TYPING   = 1 << 14;
        const MESSAGE_CONTENT         = 1 << 15; // Privileged
    }
}
```

Privileged intents require approval (admin-configurable per bot).

## Acceptance Criteria

- [ ] Bot account creation with token
- [ ] Bot authentication via Bot token
- [ ] OAuth2 flow: authorize → token exchange → bot added to server
- [ ] Incoming webhooks: create, execute, manage
- [ ] Webhook message delivery with custom username/avatar
- [ ] Slash command registration (global and per-server)
- [ ] Slash command execution → bot receives INTERACTION_CREATE
- [ ] Bot responds to interaction within 3 seconds
- [ ] Deferred responses work (acknowledge → followup)
- [ ] Message components (buttons, selects) trigger interactions
- [ ] Bot rate limits separate from user rate limits
- [ ] Gateway intents filter events correctly
- [ ] Privileged intents require admin approval
- [ ] Integration test: register command → invoke → bot responds → verify message
