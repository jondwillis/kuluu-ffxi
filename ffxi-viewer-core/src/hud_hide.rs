use std::collections::HashMap;

use bevy::prelude::*;

#[derive(Resource, Default)]
pub struct HudHidden(pub bool);

#[derive(Component)]
pub struct HudHideExempt;

#[derive(Resource, Default)]
pub struct HudHideStash(HashMap<Entity, Visibility>);

pub type HudRootFilter = (With<Node>, Without<ChildOf>, Without<HudHideExempt>);

pub fn apply_hud_hidden(
    hidden: Res<HudHidden>,
    mut stash: ResMut<HudHideStash>,
    mut roots: Query<(Entity, &mut Visibility), HudRootFilter>,
) {
    if hidden.0 {
        for (entity, mut vis) in roots.iter_mut() {
            if *vis != Visibility::Hidden {
                stash.0.entry(entity).or_insert(*vis);
                *vis = Visibility::Hidden;
            }
        }
    } else if !stash.0.is_empty() {
        for (entity, mut vis) in roots.iter_mut() {
            if let Some(prev) = stash.0.get(&entity) {
                *vis = *prev;
            }
        }
        stash.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let mut app = App::new();
        app.init_resource::<HudHidden>();
        app.init_resource::<HudHideStash>();
        app.add_systems(Update, apply_hud_hidden);
        app
    }

    fn vis(app: &App, e: Entity) -> Visibility {
        *app.world().get::<Visibility>(e).unwrap()
    }

    #[test]
    fn hides_hud_roots_and_restores_prior_visibility() {
        let mut app = test_app();
        let shown = app
            .world_mut()
            .spawn((Node::default(), Visibility::Inherited))
            .id();
        let closed = app
            .world_mut()
            .spawn((Node::default(), Visibility::Hidden))
            .id();

        app.world_mut().resource_mut::<HudHidden>().0 = true;
        app.update();
        assert_eq!(vis(&app, shown), Visibility::Hidden);
        assert_eq!(vis(&app, closed), Visibility::Hidden);

        app.world_mut().resource_mut::<HudHidden>().0 = false;
        app.update();
        assert_eq!(vis(&app, shown), Visibility::Inherited);
        assert_eq!(vis(&app, closed), Visibility::Hidden);
    }

    #[test]
    fn exempt_and_child_nodes_stay_visible() {
        let mut app = test_app();
        let exempt = app
            .world_mut()
            .spawn((Node::default(), Visibility::Inherited, HudHideExempt))
            .id();
        let root = app
            .world_mut()
            .spawn((Node::default(), Visibility::Inherited))
            .id();
        let child = app
            .world_mut()
            .spawn((Node::default(), Visibility::Inherited, ChildOf(root)))
            .id();

        app.world_mut().resource_mut::<HudHidden>().0 = true;
        app.update();
        assert_eq!(vis(&app, exempt), Visibility::Inherited);
        assert_eq!(vis(&app, root), Visibility::Hidden);
        assert_eq!(vis(&app, child), Visibility::Inherited);
    }

    #[test]
    fn root_spawned_while_hidden_is_hidden_then_restored() {
        let mut app = test_app();
        app.world_mut().resource_mut::<HudHidden>().0 = true;
        app.update();

        let late = app
            .world_mut()
            .spawn((Node::default(), Visibility::Inherited))
            .id();
        app.update();
        assert_eq!(vis(&app, late), Visibility::Hidden);

        app.world_mut().resource_mut::<HudHidden>().0 = false;
        app.update();
        assert_eq!(vis(&app, late), Visibility::Inherited);
    }

    #[test]
    fn reasserts_hidden_over_external_writes_while_hidden() {
        let mut app = test_app();
        let root = app
            .world_mut()
            .spawn((Node::default(), Visibility::Inherited))
            .id();
        app.world_mut().resource_mut::<HudHidden>().0 = true;
        app.update();

        *app.world_mut().get_mut::<Visibility>(root).unwrap() = Visibility::Visible;
        app.update();
        assert_eq!(vis(&app, root), Visibility::Hidden);

        app.world_mut().resource_mut::<HudHidden>().0 = false;
        app.update();
        assert_eq!(vis(&app, root), Visibility::Inherited);
    }
}
