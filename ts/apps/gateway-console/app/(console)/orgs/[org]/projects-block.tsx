// SPDX-License-Identifier: Apache-2.0
//
// The Projects section of the organization page (SMA-636 spec § 4.1, § 4.2): the organization's
// projects, grouped by team. A team is a grouping label only (D4); a project links to its settings
// page with prefetch={false} (§ 4.1). At most 50 teams, and 50 projects per team, show; a full list
// says "More exist. See the IAM zone." A plain async FUNCTION, awaited by the page, because its
// error branch awaits SectionError.
import type { ReactElement } from 'react';
import { ZoneLink } from '@paigasus/app-shell';
import { EmptyState } from '@paigasus/ui';
import { SectionError } from '../../../_components/section-error';
import { GATEWAY_BASE_PATH } from '../../../../lib/nav';
import type { ProjectsView, TeamGroup } from './load';

const MORE = 'More exist. See the IAM zone.';

function TeamProjects({ orgId, team }: { readonly orgId: string; readonly team: TeamGroup }): ReactElement {
  return (
    <div data-testid="project-group" className="flex flex-col gap-1">
      <h3 className="text-sm font-medium">{team.name}</h3>
      {team.projects.length === 0 ? (
        <p className="text-muted-foreground text-sm">No projects.</p>
      ) : (
        <ul className="flex flex-col gap-1 text-sm">
          {team.projects.map((project) => (
            <li key={project.projectId ?? project.slug}>
              {project.projectId === null ? (
                project.name
              ) : (
                <ZoneLink prefetch={false} href={`${GATEWAY_BASE_PATH}/orgs/${orgId}/projects/${project.projectId}`} className="hover:underline">
                  {project.name}
                </ZoneLink>
              )}
            </li>
          ))}
        </ul>
      )}
      {team.moreProjects ? (
        <p data-testid="more-projects" className="text-muted-foreground text-xs">
          {MORE}
        </p>
      ) : null}
    </div>
  );
}

export async function projectsBlock({ orgId, projects }: { readonly orgId: string; readonly projects: ProjectsView }): Promise<ReactElement> {
  let body: ReactElement;
  if (projects.kind === 'error') {
    body = await SectionError({ error: projects.error });
  } else if (projects.teams.length === 0) {
    body = <EmptyState title="No teams yet" />;
  } else {
    body = (
      <div className="flex flex-col gap-4">
        {projects.teams.map((team) => (
          <TeamProjects key={team.teamId ?? team.name} orgId={orgId} team={team} />
        ))}
        {projects.moreTeams ? (
          <p data-testid="more-teams" className="text-muted-foreground text-xs">
            {MORE}
          </p>
        ) : null}
      </div>
    );
  }
  return (
    <section aria-labelledby="projects-heading" data-testid="projects" className="flex flex-col gap-3">
      <h2 id="projects-heading" className="text-lg font-semibold">
        Projects
      </h2>
      {body}
    </section>
  );
}
