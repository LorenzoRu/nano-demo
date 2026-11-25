use std::fmt;
use std::path::{Path, PathBuf};

use crate::db::ProjectConfig;

/// Type d'accès demandé par un outil.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessKind {
    Read,
    Write,
}

/// Erreur d'accès à un chemin du projet.
/// Les variantes `NoProject`, `OutsideRoot` et `Excluded` sont des refus de
/// sécurité : l'adaptateur les marque comme fatales pour la boucle agent.
#[derive(Debug)]
pub enum AccessError {
    NoProject,
    OutsideRoot(String),
    Excluded(String),
    Invalid(String),
}

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoProject => write!(
                f,
                "Aucun dossier de projet lié. Ouvre le panneau Projet pour en choisir un."
            ),
            Self::OutsideRoot(path) => {
                write!(f, "Accès refusé : {path} est hors du dossier de projet.")
            }
            Self::Excluded(path) => write!(f, "Accès refusé : {path} est exclu du projet."),
            Self::Invalid(reason) => write!(f, "{reason}"),
        }
    }
}

/// Politique de permissions du dossier de projet.
///
/// Le cœur (`nano-core`) reste sans état : cette logique vit dans l'adaptateur Tauri.
/// Tous les chemins sont canonicalisés pour empêcher les sorties par `..` ou par lien symbolique.
#[derive(Debug)]
pub struct ProjectPolicy {
    root: PathBuf,
    exclusions: Vec<PathBuf>,
}

impl ProjectPolicy {
    pub fn new(project: &ProjectConfig) -> Result<Self, AccessError> {
        let root = project
            .root_path
            .as_deref()
            .ok_or(AccessError::NoProject)?;
        let root = std::fs::canonicalize(root).map_err(|e| {
            AccessError::Invalid(format!("Dossier de projet inaccessible ({root}) : {e}"))
        })?;

        let exclusions = project
            .exclusions
            .iter()
            .map(|exclusion| absolute_from(&root, exclusion))
            .collect();

        Ok(Self { root, exclusions })
    }

    /// Résout un chemin demandé par le modèle et vérifie qu'il reste dans le projet.
    /// Pour une écriture, le fichier peut ne pas encore exister : on canonicalise alors son parent.
    pub fn resolve(&self, requested: &str, kind: AccessKind) -> Result<PathBuf, AccessError> {
        let requested = requested.trim();
        if requested.is_empty() {
            return Err(AccessError::Invalid("Chemin vide.".to_string()));
        }

        let joined = if Path::new(requested).is_absolute() {
            PathBuf::from(requested)
        } else {
            self.root.join(requested)
        };

        let resolved = match std::fs::canonicalize(&joined) {
            Ok(path) => path,
            Err(_) if kind == AccessKind::Write => {
                let file_name = joined.file_name().ok_or_else(|| {
                    AccessError::Invalid(format!("Chemin invalide : {requested}"))
                })?;
                let parent = joined.parent().ok_or_else(|| {
                    AccessError::Invalid(format!("Chemin invalide : {requested}"))
                })?;
                let parent = std::fs::canonicalize(parent).map_err(|_| {
                    AccessError::Invalid(format!("Dossier parent introuvable pour {requested}."))
                })?;
                parent.join(file_name)
            }
            Err(_) => {
                return Err(AccessError::Invalid(format!("Chemin introuvable : {requested}.")));
            }
        };

        if !resolved.starts_with(&self.root) {
            return Err(AccessError::OutsideRoot(requested.to_string()));
        }

        if self.is_excluded(&resolved) {
            return Err(AccessError::Excluded(requested.to_string()));
        }

        Ok(resolved)
    }

    /// Indique si un chemin (déjà dans le projet) correspond à une exclusion.
    pub fn is_excluded(&self, path: &Path) -> bool {
        let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.exclusions.iter().any(|excluded| {
            resolved == *excluded || resolved.starts_with(excluded)
        })
    }
}

/// Convertit une exclusion (relative au projet ou absolue) en chemin absolu.
/// Une exclusion peut viser un élément qui n'existe plus : on retombe alors sur le chemin lexical.
fn absolute_from(root: &Path, exclusion: &str) -> PathBuf {
    let exclusion = Path::new(exclusion);
    let joined = if exclusion.is_absolute() {
        exclusion.to_path_buf()
    } else {
        root.join(exclusion)
    };

    std::fs::canonicalize(&joined).unwrap_or(joined)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_project(name: &str) -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "nano-project-test-{name}-{}-{id}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn config(root: &Path, exclusions: &[&str]) -> ProjectConfig {
        ProjectConfig {
            root_path: Some(root.to_string_lossy().to_string()),
            exclusions: exclusions.iter().map(|value| value.to_string()).collect(),
        }
    }

    #[test]
    fn refuse_without_project() {
        let error = ProjectPolicy::new(&ProjectConfig::default()).unwrap_err();
        assert!(error.to_string().contains("Aucun dossier de projet"));
    }

    #[test]
    fn accepte_un_fichier_du_projet() {
        let root = temp_project("read");
        let file = root.join("notes.md");
        std::fs::write(&file, "bonjour").unwrap();

        let policy = ProjectPolicy::new(&config(&root, &[])).unwrap();
        let resolved = policy.resolve("notes.md", AccessKind::Read).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(&file).unwrap());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn refuse_un_chemin_hors_projet() {
        let root = temp_project("outside");
        let outside = temp_project("outside-target");
        let file = outside.join("secret.txt");
        std::fs::write(&file, "secret").unwrap();

        let policy = ProjectPolicy::new(&config(&root, &[])).unwrap();
        let error = policy
            .resolve(file.to_string_lossy().as_ref(), AccessKind::Read)
            .unwrap_err();
        assert!(error.to_string().contains("hors du dossier de projet"));

        // Tentative de sortie par `..` vers le dossier frère réellement créé.
        let outside_name = outside.file_name().unwrap().to_string_lossy();
        let error = policy
            .resolve(&format!("../{outside_name}"), AccessKind::Read)
            .unwrap_err();
        assert!(error.to_string().contains("hors du dossier de projet"));

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn refuse_un_fichier_exclu() {
        let root = temp_project("excluded");
        std::fs::create_dir_all(root.join("prive")).unwrap();
        std::fs::write(root.join("prive/secret.txt"), "secret").unwrap();
        std::fs::write(root.join("public.txt"), "public").unwrap();

        let policy = ProjectPolicy::new(&config(&root, &["prive"])).unwrap();
        assert!(policy.resolve("public.txt", AccessKind::Read).is_ok());
        let error = policy.resolve("prive/secret.txt", AccessKind::Read).unwrap_err();
        assert!(error.to_string().contains("exclu du projet"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn accepte_une_ecriture_de_fichier_nouveau() {
        let root = temp_project("write");
        std::fs::create_dir_all(root.join("src")).unwrap();

        let policy = ProjectPolicy::new(&config(&root, &[])).unwrap();
        let resolved = policy.resolve("src/nouveau.txt", AccessKind::Write).unwrap();
        assert_eq!(
            resolved,
            std::fs::canonicalize(root.join("src")).unwrap().join("nouveau.txt")
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn refuse_une_sortie_par_lien_symbolique() {
        let root = temp_project("symlink");
        let outside = temp_project("symlink-target");
        std::fs::write(outside.join("secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("lien")).unwrap();

        let policy = ProjectPolicy::new(&config(&root, &[])).unwrap();
        let error = policy.resolve("lien/secret.txt", AccessKind::Read).unwrap_err();
        assert!(error.to_string().contains("hors du dossier de projet"));

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&outside).ok();
    }
}
