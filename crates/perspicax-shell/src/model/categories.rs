//! The groups a menu sorts applications into: the main categories of the
//! XDG Desktop Menu Specification, each under the name a person knows it by.
//!
//! An application goes under the first main category its entry lists, so
//! `Categories=GTK;Utility;TextEditor;` is an accessory. Audio and video
//! programs share one group, as the spec has `Audio` and `Video` each
//! require `AudioVideo`. An application with no main category goes under
//! Other.

/// A group of applications.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Category {
    // In the order a menu lists them: by name, then Other last.
    Accessories,
    Development,
    Education,
    Games,
    Graphics,
    Internet,
    Multimedia,
    Office,
    Science,
    Settings,
    System,
    Other,
}

impl Category {
    /// Every group, in the order a menu lists them.
    pub(crate) const ALL: [Self; 12] = [
        Self::Accessories,
        Self::Development,
        Self::Education,
        Self::Games,
        Self::Graphics,
        Self::Internet,
        Self::Multimedia,
        Self::Office,
        Self::Science,
        Self::Settings,
        Self::System,
        Self::Other,
    ];

    /// The group of an application whose entry lists `categories`.
    pub(crate) fn of(categories: &[String]) -> Self {
        categories
            .iter()
            .find_map(|category| Self::main(category))
            .unwrap_or(Self::Other)
    }

    /// The group a main category is, or `None` for an additional one.
    fn main(category: &str) -> Option<Self> {
        Some(match category {
            "AudioVideo" | "Audio" | "Video" => Self::Multimedia,
            "Development" => Self::Development,
            "Education" => Self::Education,
            "Game" => Self::Games,
            "Graphics" => Self::Graphics,
            "Network" => Self::Internet,
            "Office" => Self::Office,
            "Science" => Self::Science,
            "Settings" => Self::Settings,
            "System" => Self::System,
            "Utility" => Self::Accessories,
            _ => return None,
        })
    }

    /// What a menu calls it.
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Accessories => "Accessories",
            Self::Development => "Development",
            Self::Education => "Education",
            Self::Games => "Games",
            Self::Graphics => "Graphics",
            Self::Internet => "Internet",
            Self::Multimedia => "Multimedia",
            Self::Office => "Office",
            Self::Science => "Science",
            Self::Settings => "Settings",
            Self::System => "System",
            Self::Other => "Other",
        }
    }

    /// Its icon's name, as the icon naming spec has it.
    pub(crate) const fn icon(self) -> &'static str {
        match self {
            Self::Accessories => "applications-accessories",
            Self::Development => "applications-development",
            Self::Education => "applications-education",
            Self::Games => "applications-games",
            Self::Graphics => "applications-graphics",
            Self::Internet => "applications-internet",
            Self::Multimedia => "applications-multimedia",
            Self::Office => "applications-office",
            Self::Science => "applications-science",
            Self::Settings => "preferences-system",
            Self::System => "applications-system",
            Self::Other => "applications-other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(categories: &str) -> Category {
        Category::of(&categories.split(';').map(str::to_owned).collect::<Vec<_>>())
    }

    #[test]
    fn an_application_goes_under_its_first_main_category() {
        assert_eq!(of("GTK;Utility;TextEditor"), Category::Accessories);
        assert_eq!(
            of("System;Settings"),
            Category::System,
            "the first, not the last"
        );
        assert_eq!(of("Qt;KDE;Video;Player"), Category::Multimedia);
        assert_eq!(of("Network;WebBrowser"), Category::Internet);
        assert_eq!(of("TextEditor"), Category::Other, "only additional ones");
        assert_eq!(of(""), Category::Other);
    }

    #[test]
    fn the_groups_are_listed_by_name_with_other_last() {
        let labels: Vec<_> = Category::ALL.iter().map(|c| c.label()).collect();
        let mut sorted = labels[..labels.len() - 1].to_vec();
        sorted.sort_unstable();
        assert_eq!(labels[..labels.len() - 1], sorted);
        assert_eq!(labels.last(), Some(&"Other"));
    }
}
