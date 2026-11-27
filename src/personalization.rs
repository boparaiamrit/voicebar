//! Pasted transcripts and approved examples for the local rewrite profile.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_TRANSCRIPT_BYTES: usize = 4096;
pub const PREVIEW_COUNT: usize = 3;
const MAX_PASTE_BYTES: usize = 128 * 1024;
const MAX_PASTED_TRANSCRIPTS: usize = 50;
const MAX_REVIEW_SAMPLES: usize = 5;
const MAX_PROFILE_BYTES: usize = 4096;

fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn valid_transcript(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= MAX_TRANSCRIPT_BYTES
}

fn split_paste(text: &str) -> Result<Vec<String>, &'static str> {
    if text.len() > MAX_PASTE_BYTES {
        return Err("This paste is too large. Paste a smaller batch of transcripts.");
    }

    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    let mut start = 0;
    let mut end = 0;
    for line in text.split_inclusive('\n').chain(std::iter::once("")) {
        if line.trim().is_empty() {
            let block = text[start..end].trim();
            if !block.is_empty() && seen.insert(normalized(block)) {
                if entries.len() == MAX_PASTED_TRANSCRIPTS {
                    return Err("Paste up to 50 distinct dictations at a time.");
                }
                if !valid_transcript(block) {
                    return Err(
                        "One dictation exceeds 4,096 UTF-8 bytes. Shorten it or split it at a natural break.",
                    );
                }
                entries.push(block.to_owned());
            }
            start = end + line.len();
        }
        end += line.len();
    }
    Ok(entries)
}

pub fn count_paste_entries(text: &str) -> Result<usize, &'static str> {
    Ok(split_paste(text)?.len())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Example {
    pub original: String,
    pub preferred: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewriteProfile {
    examples: Vec<Example>,
}

impl RewriteProfile {
    pub fn examples(&self) -> &[Example] {
        &self.examples
    }

    pub fn text_bytes(&self) -> usize {
        self.examples.iter().map(Example::text_bytes).sum()
    }

    pub fn is_valid(&self) -> bool {
        !self.examples.is_empty()
            && self.examples.len() <= MAX_REVIEW_SAMPLES
            && self.examples.iter().all(|example| {
                valid_transcript(&example.original) && valid_transcript(&example.preferred)
            })
            && self.text_bytes() <= MAX_PROFILE_BYTES
    }
}

impl Example {
    fn text_bytes(&self) -> usize {
        self.original.len() + self.preferred.len()
    }
}

#[derive(Debug)]
pub struct SetupDraft {
    originals: Vec<String>,
    reviewed: usize,
    profile: RewriteProfile,
}

impl SetupDraft {
    pub fn from_paste(text: &str) -> Result<Self, &'static str> {
        Self::from_entries(split_paste(text)?)
    }

    pub fn from_entries(entries: Vec<String>) -> Result<Self, &'static str> {
        let mut seen = HashSet::new();
        let originals: Vec<_> = entries
            .into_iter()
            .filter(|text| valid_transcript(text) && seen.insert(normalized(text)))
            .collect();
        if originals.is_empty() {
            return Err("Add at least one transcript to continue.");
        }
        if originals.len() > MAX_PASTED_TRANSCRIPTS {
            return Err("Use up to 50 distinct transcripts at a time.");
        }
        Ok(Self {
            originals,
            reviewed: 0,
            profile: RewriteProfile::default(),
        })
    }

    pub fn sample_count(&self) -> usize {
        self.originals.len()
    }

    pub fn approved_count(&self) -> usize {
        self.profile.examples.len()
    }

    pub fn used_bytes(&self) -> usize {
        self.profile.text_bytes()
    }

    pub const fn max_profile_bytes() -> usize {
        MAX_PROFILE_BYTES
    }

    pub fn restart_review(&mut self) {
        self.reviewed = 0;
        self.profile = RewriteProfile::default();
    }

    pub fn review_position(&self) -> (usize, usize) {
        let total = self.originals.len().min(MAX_REVIEW_SAMPLES);
        ((self.reviewed + 1).min(total), total)
    }

    pub fn current(&self) -> Option<&str> {
        let (_, total) = self.review_position();
        if self.reviewed >= total {
            return None;
        }
        // In a larger paste, review samples from across the batch.
        let index = if total < 2 {
            0
        } else {
            self.reviewed * (self.originals.len() - 1) / (total - 1)
        };
        Some(&self.originals[index])
    }

    pub fn review(&mut self, preferred: Option<&str>) -> Result<(), &'static str> {
        let original = self.current().ok_or("Review is already complete.")?;
        if let Some(preferred) = preferred {
            let preferred = preferred.trim();
            if !valid_transcript(preferred) {
                return Err("Enter the wording you would send, up to 4,096 UTF-8 bytes.");
            }
            let example = Example {
                original: original.into(),
                preferred: preferred.into(),
            };
            let used = self
                .profile
                .examples
                .iter()
                .map(Example::text_bytes)
                .sum::<usize>();
            if used + example.text_bytes() > MAX_PROFILE_BYTES {
                return Err(
                    "These examples are too long for the profile. Shorten the wording, skip this sample, or use a shorter original.",
                );
            }
            self.profile.examples.push(example);
        } else if self.reviewed + 1 == self.review_position().1 && self.profile.examples.is_empty()
        {
            return Err("Choose at least one rewrite or original to teach VoiceBar.");
        }
        self.reviewed += 1;
        Ok(())
    }

    pub fn preview_inputs(&self, text: &str) -> Result<PreviewInputs, &'static str> {
        if self.current().is_some() || !self.profile.is_valid() {
            return Err("Finish reviewing examples before you compare rewrites.");
        }
        let originals: [String; PREVIEW_COUNT] = split_paste(text)?.try_into().map_err(
            |_| "Paste exactly three different dictations, with a blank line between them.",
        )?;
        let previous: HashSet<_> = self.originals.iter().map(|text| normalized(text)).collect();
        if originals
            .iter()
            .any(|text| previous.contains(&normalized(text)))
        {
            return Err("Use three new dictations that were outside the original paste.");
        }
        Ok(PreviewInputs {
            profile: self.profile.clone(),
            originals,
        })
    }
}

#[derive(Debug)]
pub struct PreviewInputs {
    pub profile: RewriteProfile,
    pub originals: [String; PREVIEW_COUNT],
}

#[derive(Debug)]
pub struct RewritePair {
    pub standard: String,
    pub personalized: String,
}

#[derive(Debug)]
pub struct ProfilePreview {
    pub inputs: PreviewInputs,
    pub rewrites: [RewritePair; PREVIEW_COUNT],
}

impl ProfilePreview {
    pub fn complete(
        inputs: PreviewInputs,
        rewrites: Vec<RewritePair>,
    ) -> Result<Self, &'static str> {
        let rewrites: [RewritePair; PREVIEW_COUNT] = rewrites
            .try_into()
            .map_err(|_| "VoiceBar needs three complete comparisons.")?;
        if rewrites
            .iter()
            .any(|pair| pair.standard.trim().is_empty() || pair.personalized.trim().is_empty())
        {
            return Err("VoiceBar needs a complete rewrite from both versions.");
        }
        Ok(Self { inputs, rewrites })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review_one() -> SetupDraft {
        let mut draft = SetupDraft::from_paste("Can you uh check it?").unwrap();
        draft.review(Some("Can you check it?")).unwrap();
        draft
    }

    #[test]
    fn parses_distinct_paragraphs_and_keeps_single_line_breaks() {
        let mut draft =
            SetupDraft::from_paste("  First\r\nline\r\n \r\nSecond\r\n\r\n FIRST line  ").unwrap();
        assert_eq!(draft.sample_count(), 2);
        assert_eq!(draft.current(), Some("First\nline"));
        draft.review(Some("First line.")).unwrap();
        assert_eq!(draft.current(), Some("Second"));
        draft.review(None).unwrap();
        let inputs = draft.preview_inputs("Third\n\nFourth\n\nFifth").unwrap();
        assert_eq!(inputs.profile.examples().len(), 1);
    }

    #[test]
    fn rejects_blank_and_oversized_pastes_without_truncating() {
        assert!(SetupDraft::from_paste(" \n\n ").is_err());
        assert!(SetupDraft::from_paste(&"é".repeat(MAX_TRANSCRIPT_BYTES)).is_err());
    }

    #[test]
    fn uses_up_to_five_review_samples_across_the_paste() {
        let draft =
            SetupDraft::from_paste("One\n\nTwo\n\nThree\n\nFour\n\nFive\n\nSix\n\nSeven").unwrap();
        assert_eq!(draft.sample_count(), 7);
        assert_eq!(draft.review_position(), (1, 5));
    }

    #[test]
    fn requires_a_reviewed_choice_and_three_unseen_inputs() {
        let mut draft = SetupDraft::from_paste("Sample").unwrap();
        assert!(draft.preview_inputs("Other\n\nNext\n\nThird").is_err());
        assert!(draft.review(None).is_err());

        let draft = review_one();
        for invalid in [
            "Other\n\nNext",
            "Other\n\nNext\n\nThird\n\nFourth",
            "Other\n\n other \n\nNext",
            "Other\n\nCan  you uh check it?\n\nThird",
        ] {
            assert!(draft.preview_inputs(invalid).is_err(), "{invalid:?}");
        }
        let inputs = draft.preview_inputs("Other\n\nNext\n\nThird").unwrap();
        assert_eq!(inputs.originals, ["Other", "Next", "Third"]);
        assert!(inputs.profile.is_valid());
    }

    #[test]
    fn rejects_a_profile_too_long_for_the_model_context_before_advancing() {
        let long = "x".repeat(3500);
        let mut draft = SetupDraft::from_paste("One\n\nTwo").unwrap();
        draft.review(Some(&long)).unwrap();
        assert!(draft.review(Some(&long)).is_err());
        assert_eq!(draft.current(), Some("Two"));
        draft.review(Some("Two")).unwrap();
        assert_eq!(
            draft
                .preview_inputs("Three\n\nFour\n\nFive")
                .unwrap()
                .profile
                .examples()
                .len(),
            2
        );
    }

    #[test]
    fn requires_three_complete_pairs_and_preserves_the_profile_reviewed() {
        let draft = review_one();
        let inputs = || draft.preview_inputs("Second\n\nThird\n\nFourth").unwrap();
        let pair = |standard: &str| RewritePair {
            standard: standard.to_string(),
            personalized: "Personalized".into(),
        };
        assert!(ProfilePreview::complete(inputs(), vec![pair("Only one")]).is_err());
        assert!(
            ProfilePreview::complete(inputs(), vec![pair("Valid"), pair(" "), pair("Valid")])
                .is_err()
        );
        let preview =
            ProfilePreview::complete(inputs(), vec![pair("First"), pair("Next"), pair("Last")])
                .unwrap();
        assert_eq!(
            preview.inputs.profile.examples()[0].preferred,
            "Can you check it?"
        );
        assert_eq!(preview.rewrites[2].standard, "Last");
    }
}
