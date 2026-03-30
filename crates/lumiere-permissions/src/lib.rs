use bitflags::bitflags;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Permissions: u64 {
        // General
        const ADMINISTRATOR           = 1 << 0;
        const VIEW_AUDIT_LOG          = 1 << 1;
        const MANAGE_SERVER           = 1 << 2;
        const MANAGE_ROLES            = 1 << 3;
        const MANAGE_CHANNELS         = 1 << 4;
        const KICK_MEMBERS            = 1 << 5;
        const BAN_MEMBERS             = 1 << 6;
        const CREATE_INVITE           = 1 << 7;
        const CHANGE_NICKNAME         = 1 << 8;
        const MANAGE_NICKNAMES        = 1 << 9;
        const MANAGE_EMOJIS           = 1 << 10;
        const MANAGE_WEBHOOKS         = 1 << 11;
        const VIEW_CHANNEL            = 1 << 12;
        const MODERATE_MEMBERS        = 1 << 13;

        // Text
        const SEND_MESSAGES           = 1 << 14;
        const SEND_TTS_MESSAGES       = 1 << 15;
        const MANAGE_MESSAGES         = 1 << 16;
        const EMBED_LINKS             = 1 << 17;
        const ATTACH_FILES            = 1 << 18;
        const READ_MESSAGE_HISTORY    = 1 << 19;
        const MENTION_EVERYONE        = 1 << 20;
        const USE_EXTERNAL_EMOJIS     = 1 << 21;
        const ADD_REACTIONS           = 1 << 22;
        const USE_SLASH_COMMANDS      = 1 << 23;
        const SEND_MESSAGES_IN_THREADS = 1 << 24;
        const CREATE_PUBLIC_THREADS   = 1 << 25;
        const CREATE_PRIVATE_THREADS  = 1 << 26;
        const MANAGE_THREADS          = 1 << 27;

        // Voice
        const CONNECT                 = 1 << 28;
        const SPEAK                   = 1 << 29;
        const MUTE_MEMBERS            = 1 << 30;
        const DEAFEN_MEMBERS          = 1 << 31;
        const MOVE_MEMBERS            = 1 << 32;
        const USE_VAD                 = 1 << 33;
        const PRIORITY_SPEAKER        = 1 << 34;
        const STREAM                  = 1 << 35;
        const USE_SOUNDBOARD          = 1 << 36;

        // Stage
        const REQUEST_TO_SPEAK        = 1 << 37;
    }
}

impl Permissions {
    /// Default permissions for @everyone role
    pub fn default_everyone() -> Self {
        Self::VIEW_CHANNEL
            | Self::SEND_MESSAGES
            | Self::READ_MESSAGE_HISTORY
            | Self::EMBED_LINKS
            | Self::ATTACH_FILES
            | Self::ADD_REACTIONS
            | Self::USE_EXTERNAL_EMOJIS
            | Self::CONNECT
            | Self::SPEAK
            | Self::USE_VAD
            | Self::CHANGE_NICKNAME
            | Self::CREATE_INVITE
    }
}

/// Permission override for a channel
#[derive(Debug, Clone)]
pub struct PermissionOverride {
    /// Role ID or User ID
    pub target_id: u64,
    /// 0 = role, 1 = member
    pub target_type: u8,
    pub allow: Permissions,
    pub deny: Permissions,
}

/// Compute the final permissions for a member in a server, optionally in a specific channel.
///
/// Follows the exact Discord algorithm:
/// 1. Server owner → all permissions
/// 2. Base permissions from roles (OR together)
/// 3. ADMINISTRATOR → all permissions
/// 4. Channel overrides: @everyone → role overrides → member override
pub fn compute_permissions(
    is_owner: bool,
    member_role_ids: &[u64],
    everyone_role_permissions: u64,
    role_permissions: &[(u64, u64)], // (role_id, permissions_bits)
    channel_overrides: Option<&[PermissionOverride]>,
    member_id: u64,
    server_id: u64,
) -> Permissions {
    // Step 1: Server owner has ALL permissions
    if is_owner {
        return Permissions::all();
    }

    // Step 2: Calculate base permissions from roles
    let mut permissions = Permissions::from_bits_truncate(everyone_role_permissions);

    for role_id in member_role_ids {
        if let Some((_, bits)) = role_permissions.iter().find(|(id, _)| id == role_id) {
            permissions |= Permissions::from_bits_truncate(*bits);
        }
    }

    // Step 3: ADMINISTRATOR grants everything
    if permissions.contains(Permissions::ADMINISTRATOR) {
        return Permissions::all();
    }

    // Step 4: Apply channel overrides
    if let Some(overrides) = channel_overrides {
        // 4a: Apply @everyone role override
        if let Some(ov) = overrides
            .iter()
            .find(|o| o.target_id == server_id && o.target_type == 0)
        {
            permissions &= !ov.deny;
            permissions |= ov.allow;
        }

        // 4b: Apply role overrides (aggregate)
        let mut role_allow = Permissions::empty();
        let mut role_deny = Permissions::empty();
        for role_id in member_role_ids {
            if let Some(ov) = overrides
                .iter()
                .find(|o| o.target_id == *role_id && o.target_type == 0)
            {
                role_allow |= ov.allow;
                role_deny |= ov.deny;
            }
        }
        permissions &= !role_deny;
        permissions |= role_allow;

        // 4c: Apply member-specific override (highest priority)
        if let Some(ov) = overrides
            .iter()
            .find(|o| o.target_id == member_id && o.target_type == 1)
        {
            permissions &= !ov.deny;
            permissions |= ov.allow;
        }
    }

    permissions
}

/// Check if an actor can modify a target based on role hierarchy.
/// Returns true if the actor's highest role position is above the target's.
pub fn can_modify_member(actor_highest_position: i32, target_highest_position: i32) -> bool {
    actor_highest_position > target_highest_position
}

/// Get the highest role position for a set of role IDs.
pub fn highest_role_position(member_role_ids: &[u64], all_roles: &[(u64, i32)]) -> i32 {
    member_role_ids
        .iter()
        .filter_map(|id| {
            all_roles
                .iter()
                .find(|(rid, _)| rid == id)
                .map(|(_, pos)| *pos)
        })
        .max()
        .unwrap_or(0)
}

impl fmt::Display for Permissions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.bits())
    }
}

/// Serialize as string (large numbers)
impl Serialize for Permissions {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.bits().to_string())
    }
}

impl<'de> Deserialize<'de> for Permissions {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let bits: u64 = s.parse().map_err(serde::de::Error::custom)?;
        Ok(Permissions::from_bits_truncate(bits))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_owner_has_all_permissions() {
        let perms = compute_permissions(true, &[], 0, &[], None, 1, 100);
        assert_eq!(perms, Permissions::all());
    }

    #[test]
    fn test_base_permissions_from_roles() {
        let everyone = Permissions::VIEW_CHANNEL.bits();
        let role_perms = vec![
            (2, Permissions::SEND_MESSAGES.bits()),
            (3, Permissions::MANAGE_MESSAGES.bits()),
        ];

        let perms = compute_permissions(false, &[2, 3], everyone, &role_perms, None, 1, 100);
        assert!(perms.contains(Permissions::VIEW_CHANNEL));
        assert!(perms.contains(Permissions::SEND_MESSAGES));
        assert!(perms.contains(Permissions::MANAGE_MESSAGES));
    }

    #[test]
    fn test_administrator_grants_all() {
        let everyone = Permissions::ADMINISTRATOR.bits();
        let perms = compute_permissions(false, &[], everyone, &[], None, 1, 100);
        assert_eq!(perms, Permissions::all());
    }

    #[test]
    fn test_channel_override_deny() {
        let everyone = (Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES).bits();
        let overrides = vec![PermissionOverride {
            target_id: 100, // server_id = @everyone
            target_type: 0,
            allow: Permissions::empty(),
            deny: Permissions::SEND_MESSAGES,
        }];

        let perms = compute_permissions(false, &[], everyone, &[], Some(&overrides), 1, 100);
        assert!(perms.contains(Permissions::VIEW_CHANNEL));
        assert!(!perms.contains(Permissions::SEND_MESSAGES));
    }

    #[test]
    fn test_member_override_takes_priority() {
        let everyone = Permissions::VIEW_CHANNEL.bits();
        let overrides = vec![
            PermissionOverride {
                target_id: 100, // deny SEND_MESSAGES for @everyone
                target_type: 0,
                allow: Permissions::empty(),
                deny: Permissions::SEND_MESSAGES,
            },
            PermissionOverride {
                target_id: 1, // allow SEND_MESSAGES for specific member
                target_type: 1,
                allow: Permissions::SEND_MESSAGES,
                deny: Permissions::empty(),
            },
        ];

        let perms = compute_permissions(false, &[], everyone, &[], Some(&overrides), 1, 100);
        assert!(perms.contains(Permissions::SEND_MESSAGES));
    }

    #[test]
    fn test_role_hierarchy() {
        let actor_roles = &[(1u64, 5i32), (2u64, 10i32)];
        let target_roles = &[(1u64, 5i32), (3u64, 8i32)];

        let actor_highest = highest_role_position(&[1, 2], actor_roles);
        let target_highest = highest_role_position(&[1, 3], target_roles);

        assert!(can_modify_member(actor_highest, target_highest));
    }

    #[test]
    fn test_default_everyone_permissions() {
        let perms = Permissions::default_everyone();
        assert!(perms.contains(Permissions::VIEW_CHANNEL));
        assert!(perms.contains(Permissions::SEND_MESSAGES));
        assert!(!perms.contains(Permissions::ADMINISTRATOR));
        assert!(!perms.contains(Permissions::MANAGE_SERVER));
    }
}
