use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    Main,
    Vision,
    Video,
    AudioStt,
    ImageGeneration,
    Curator,
}

impl ModelRole {
    #[allow(dead_code)]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "main" => Some(Self::Main),
            "vision" => Some(Self::Vision),
            "video" => Some(Self::Video),
            "audio_stt" | "audio-stt" | "stt" | "audio" => Some(Self::AudioStt),
            "image_gen" | "image-gen" | "image_generation" | "image-generation" | "image" => {
                Some(Self::ImageGeneration)
            }
            "curator" | "judge" | "memory_curator" | "memory-curator" => Some(Self::Curator),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Main => "Main Model",
            Self::Vision => "Vision Model",
            Self::Video => "Video Model",
            Self::AudioStt => "Audio STT Model",
            Self::ImageGeneration => "Image Generation Model",
            Self::Curator => "Memory Curator",
        }
    }

    pub fn addon_roles() -> [Self; 5] {
        [
            Self::Vision,
            Self::Video,
            Self::AudioStt,
            Self::ImageGeneration,
            Self::Curator,
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModelRoute {
    #[default]
    MainModel,
    Specific {
        provider_id: String,
        model: String,
    },
    Disabled,
}

impl ModelRoute {
    pub fn referenced_provider(&self) -> Option<&str> {
        match self {
            Self::Specific { provider_id, .. } => Some(provider_id),
            Self::MainModel | Self::Disabled => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteOrigin {
    Main,
    MainModel,
    Specific,
}

#[derive(Debug, Clone)]
pub struct ResolvedModelRoute {
    pub provider: crate::ai::storage::ProviderConfig,
    pub model: String,
    pub capability: crate::ai::storage::CapabilityRecord,
    pub route_origin: RouteOrigin,
}

#[derive(Debug, Clone)]
pub struct GenerationModelSnapshot {
    pub(crate) provider_store: crate::ai::storage::ProviderStore,
    pub(crate) routing: ModelRoutingConfig,
    pub(crate) capabilities: crate::ai::storage::CapabilityRegistry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRoutingConfig {
    pub version: u32,
    #[serde(default)]
    pub vision: ModelRoute,
    #[serde(default)]
    pub video: ModelRoute,
    #[serde(default)]
    pub audio_stt: ModelRoute,
    #[serde(default)]
    pub image_gen: ModelRoute,
    #[serde(default)]
    pub curator: ModelRoute,
}

impl Default for ModelRoutingConfig {
    fn default() -> Self {
        Self {
            version: 1,
            vision: ModelRoute::MainModel,
            video: ModelRoute::MainModel,
            audio_stt: ModelRoute::MainModel,
            image_gen: ModelRoute::MainModel,
            curator: ModelRoute::MainModel,
        }
    }
}

impl ModelRoutingConfig {
    pub fn route(&self, role: ModelRole) -> Option<&ModelRoute> {
        match role {
            ModelRole::Main => None,
            ModelRole::Vision => Some(&self.vision),
            ModelRole::Video => Some(&self.video),
            ModelRole::AudioStt => Some(&self.audio_stt),
            ModelRole::ImageGeneration => Some(&self.image_gen),
            ModelRole::Curator => Some(&self.curator),
        }
    }

    pub fn set_route(&mut self, role: ModelRole, route: ModelRoute) -> Result<(), String> {
        match role {
            ModelRole::Main => {
                Err("Main Model is configured through the main model selector".into())
            }
            ModelRole::Vision => {
                self.vision = route;
                Ok(())
            }
            ModelRole::Video => {
                self.video = route;
                Ok(())
            }
            ModelRole::AudioStt => {
                self.audio_stt = route;
                Ok(())
            }
            ModelRole::ImageGeneration => {
                self.image_gen = route;
                Ok(())
            }
            ModelRole::Curator => {
                self.curator = route;
                Ok(())
            }
        }
    }

    pub fn roles_using_provider(&self, provider_id: &str) -> Vec<ModelRole> {
        ModelRole::addon_roles()
            .into_iter()
            .filter(|role| {
                self.route(*role)
                    .and_then(ModelRoute::referenced_provider)
                    .is_some_and(|id| id == provider_id)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_live_main_model_routes() {
        let config = ModelRoutingConfig::default();
        for role in ModelRole::addon_roles() {
            assert_eq!(config.route(role), Some(&ModelRoute::MainModel));
        }
    }

    #[test]
    fn specific_provider_dependencies_are_detected() {
        let mut config = ModelRoutingConfig::default();
        config
            .set_route(
                ModelRole::ImageGeneration,
                ModelRoute::Specific {
                    provider_id: "together".into(),
                    model: "flux".into(),
                },
            )
            .unwrap();
        assert_eq!(
            config.roles_using_provider("together"),
            vec![ModelRole::ImageGeneration]
        );
    }

    #[test]
    fn specific_route_is_not_rewritten_when_main_changes() {
        let mut config = ModelRoutingConfig::default();
        let specific = ModelRoute::Specific {
            provider_id: "vision-provider".into(),
            model: "vision-model".into(),
        };
        config
            .set_route(ModelRole::Vision, specific.clone())
            .unwrap();
        assert_eq!(config.route(ModelRole::Vision), Some(&specific));
    }

    #[test]
    fn disabled_route_is_explicit_and_stable() {
        let mut config = ModelRoutingConfig::default();
        config
            .set_route(ModelRole::Video, ModelRoute::Disabled)
            .unwrap();
        assert_eq!(config.route(ModelRole::Video), Some(&ModelRoute::Disabled));
    }

    #[test]
    fn curator_role_parses_and_routes() {
        assert_eq!(ModelRole::parse("curator"), Some(ModelRole::Curator));
        assert_eq!(ModelRole::parse("judge"), Some(ModelRole::Curator));
        assert_eq!(ModelRole::parse("memory_curator"), Some(ModelRole::Curator));
        assert_eq!(ModelRole::Curator.display_name(), "Memory Curator");

        let mut config = ModelRoutingConfig::default();
        assert_eq!(
            config.route(ModelRole::Curator),
            Some(&ModelRoute::MainModel)
        );
        config
            .set_route(
                ModelRole::Curator,
                ModelRoute::Specific {
                    provider_id: "groq".into(),
                    model: "llama-3.1-8b-instant".into(),
                },
            )
            .unwrap();
        assert_eq!(
            config.route(ModelRole::Curator),
            Some(&ModelRoute::Specific {
                provider_id: "groq".into(),
                model: "llama-3.1-8b-instant".into(),
            })
        );
        assert_eq!(
            config.roles_using_provider("groq"),
            vec![ModelRole::Curator]
        );
    }
}
