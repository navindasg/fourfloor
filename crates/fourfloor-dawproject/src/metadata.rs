//! `metadata.xml`: the descriptive fields of a DAWproject file.

use crate::error::DawprojectError;
use crate::xml::{elements, tag, XmlWriter};

/// The fields of DAWproject's `MetaData` element. All are optional.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    /// Title of the song. Export fills in the project name when empty.
    pub title: Option<String>,
    /// Recording artist.
    pub artist: Option<String>,
    /// Album.
    pub album: Option<String>,
    /// Original artist.
    pub original_artist: Option<String>,
    /// Composer.
    pub composer: Option<String>,
    /// Songwriter.
    pub songwriter: Option<String>,
    /// Producer.
    pub producer: Option<String>,
    /// Arranger.
    pub arranger: Option<String>,
    /// Year.
    pub year: Option<String>,
    /// Genre.
    pub genre: Option<String>,
    /// Copyright notice.
    pub copyright: Option<String>,
    /// Website.
    pub website: Option<String>,
    /// Free-form comment.
    pub comment: Option<String>,
}

impl Metadata {
    /// The fields in the order the schema demands, with their element names.
    fn fields(&self) -> [(&'static str, &Option<String>); 13] {
        [
            ("Title", &self.title),
            ("Artist", &self.artist),
            ("Album", &self.album),
            ("OriginalArtist", &self.original_artist),
            ("Composer", &self.composer),
            ("Songwriter", &self.songwriter),
            ("Producer", &self.producer),
            ("Arranger", &self.arranger),
            ("Year", &self.year),
            ("Genre", &self.genre),
            ("Copyright", &self.copyright),
            ("Website", &self.website),
            ("Comment", &self.comment),
        ]
    }

    fn field_mut(&mut self, element: &str) -> Option<&mut Option<String>> {
        Some(match element {
            "Title" => &mut self.title,
            "Artist" => &mut self.artist,
            "Album" => &mut self.album,
            "OriginalArtist" => &mut self.original_artist,
            "Composer" => &mut self.composer,
            "Songwriter" => &mut self.songwriter,
            "Producer" => &mut self.producer,
            "Arranger" => &mut self.arranger,
            "Year" => &mut self.year,
            "Genre" => &mut self.genre,
            "Copyright" => &mut self.copyright,
            "Website" => &mut self.website,
            "Comment" => &mut self.comment,
            _ => return None,
        })
    }

    /// Writes the `metadata.xml` document.
    pub fn to_xml(&self) -> String {
        let mut writer = XmlWriter::new();
        writer.open(crate::xml::El::new("MetaData"));
        for (element, value) in self.fields() {
            if let Some(value) = value {
                writer.text(element, value);
            }
        }
        writer.finish()
    }

    /// Reads a `metadata.xml` document. Unknown elements are ignored.
    pub fn from_xml(text: &str) -> Result<Self, DawprojectError> {
        let document = roxmltree::Document::parse(text)
            .map_err(|error| DawprojectError::Xml(format!("metadata.xml: {error}")))?;
        let mut metadata = Self::default();
        for node in elements(document.root_element()) {
            if let Some(field) = metadata.field_mut(tag(node)) {
                *field = Some(node.text().unwrap_or_default().to_owned());
            }
        }
        Ok(metadata)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trips_in_schema_order() {
        let metadata = Metadata {
            title: Some("Jack & <Jill>".to_owned()),
            comment: Some("124 BPM".to_owned()),
            artist: Some("Nobody".to_owned()),
            ..Metadata::default()
        };
        let xml = metadata.to_xml();
        assert!(xml.find("<Title>").unwrap() < xml.find("<Artist>").unwrap());
        assert!(xml.find("<Artist>").unwrap() < xml.find("<Comment>").unwrap());
        assert_eq!(Metadata::from_xml(&xml).unwrap(), metadata);
        assert_eq!(
            Metadata::from_xml("<MetaData><Mood>x</Mood></MetaData>").unwrap(),
            Metadata::default()
        );
        assert!(Metadata::from_xml("<MetaData>").is_err());
    }
}
