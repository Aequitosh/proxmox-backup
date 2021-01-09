//! Types for tape media pool API
//!
//! Note: Both MediaSetPolicy and RetentionPolicy are complex enums,
//! so we cannot use them directly for the API. Instead, we represent
//! them as String.

use anyhow::Error;
use std::str::FromStr;
use serde::{Deserialize, Serialize};

use proxmox::api::{
    api,
    schema::{Schema, StringSchema, ApiStringFormat},
};

use crate::{
    tools::systemd::time::{
        CalendarEvent,
        TimeSpan,
        parse_time_span,
        parse_calendar_event,
    },
    api2::types::{
        DRIVE_NAME_SCHEMA,
        PROXMOX_SAFE_ID_FORMAT,
        SINGLE_LINE_COMMENT_FORMAT,
    },
};

pub const MEDIA_POOL_NAME_SCHEMA: Schema = StringSchema::new("Media pool name.")
    .format(&PROXMOX_SAFE_ID_FORMAT)
    .min_length(2)
    .max_length(32)
    .schema();

pub const MEDIA_SET_NAMING_TEMPLATE_SCHEMA: Schema = StringSchema::new(
    "Media set naming template.")
    .format(&SINGLE_LINE_COMMENT_FORMAT)
    .min_length(2)
    .max_length(64)
    .schema();

pub const MEDIA_SET_ALLOCATION_POLICY_FORMAT: ApiStringFormat =
    ApiStringFormat::VerifyFn(|s| { MediaSetPolicy::from_str(s)?; Ok(()) });

pub const MEDIA_SET_ALLOCATION_POLICY_SCHEMA: Schema = StringSchema::new(
    "Media set allocation policy.")
    .format(&MEDIA_SET_ALLOCATION_POLICY_FORMAT)
    .schema();

/// Media set allocation policy
pub enum MediaSetPolicy {
    /// Try to use the current media set
    ContinueCurrent,
    /// Each backup job creates a new media set
    AlwaysCreate,
    /// Create a new set when the specified CalendarEvent triggers
    CreateAt(CalendarEvent),
}

impl std::str::FromStr for MediaSetPolicy {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "continue" {
            return Ok(MediaSetPolicy::ContinueCurrent);
        }
        if s == "always" {
            return Ok(MediaSetPolicy::AlwaysCreate);
        }

        let event = parse_calendar_event(s)?;

        Ok(MediaSetPolicy::CreateAt(event))
    }
}

pub const MEDIA_RETENTION_POLICY_FORMAT: ApiStringFormat =
    ApiStringFormat::VerifyFn(|s| { RetentionPolicy::from_str(s)?; Ok(()) });

pub const MEDIA_RETENTION_POLICY_SCHEMA: Schema = StringSchema::new(
    "Media retention policy.")
    .format(&MEDIA_RETENTION_POLICY_FORMAT)
    .schema();

/// Media retention Policy
pub enum RetentionPolicy {
    /// Always overwrite media
    OverwriteAlways,
    /// Protect data for the timespan specified
    ProtectFor(TimeSpan),
    /// Never overwrite data
    KeepForever,
}

impl std::str::FromStr for RetentionPolicy {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "overwrite" {
            return Ok(RetentionPolicy::OverwriteAlways);
        }
        if s == "keep" {
            return Ok(RetentionPolicy::KeepForever);
        }

        let time_span = parse_time_span(s)?;

        Ok(RetentionPolicy::ProtectFor(time_span))
    }
}

#[api(
    properties: {
        name: {
            schema: MEDIA_POOL_NAME_SCHEMA,
        },
        drive: {
            schema: DRIVE_NAME_SCHEMA,
        },
        allocation: {
            schema: MEDIA_SET_ALLOCATION_POLICY_SCHEMA,
            optional: true,
        },
        retention: {
            schema: MEDIA_RETENTION_POLICY_SCHEMA,
            optional: true,
        },
        template: {
            schema: MEDIA_SET_NAMING_TEMPLATE_SCHEMA,
            optional: true,
        },
    }
)]
#[derive(Serialize,Deserialize)]
/// Media pool configuration
pub struct MediaPoolConfig {
    /// The pool name
    pub name: String,
    /// The associated drive
    pub drive: String,
    /// Media Set allocation policy
    #[serde(skip_serializing_if="Option::is_none")]
    pub allocation: Option<String>,
    /// Media retention policy
    #[serde(skip_serializing_if="Option::is_none")]
    pub retention: Option<String>,
    /// Media set naming template (default "%c")
    ///
    /// The template is UTF8 text, and can include strftime time
    /// format specifications.
    #[serde(skip_serializing_if="Option::is_none")]
    pub template: Option<String>,
}
