// Omni Automation script that authors the omniplan-native.xml content in a NEW OmniPlan document.
// Run it with: tell application id "com.omnigroup.OmniPlan4.MacAppStore" to evaluate javascript
// (contents of this file); then run OmniPlanNative.applescript for what Omni Automation does not
// expose (lead times, constraint dates) and export as described in README.md.
(function () {
  Document.makeNewAndShow(function (doc) {
    var scenario = doc.project.actual;
    doc.project.title = 'DPM OmniPlan 原生夹具';
    var root = scenario.rootTask;
    function add(parent, title, type, hours, priority) {
      var task = parent.addSubtask();
      task.title = title;
      if (type) { task.type = type; }
      if (hours !== null) { task.duration = Duration.workHours(hours); }
      if (priority !== null) { task.priority = priority; }
      return task;
    }
    var design = add(root, '设计 Design', null, null, null);
    var spec = add(design, '编写规格说明', null, 16, 3);
    var review = add(design, '审查规格', null, 8, 5);
    var approved = add(design, '设计批准', TaskType.milestone, null, null);
    design.type = TaskType.group;
    var build = add(root, '构建 Build', null, null, null);
    var impl = add(build, '实现功能', null, 40, 9);
    var docs = add(build, '编写文档', null, 12, 5);
    var test = add(build, '集成测试', null, 16, 7);
    build.type = TaskType.group;
    var release = add(root, '发布 Release', TaskType.milestone, null, null);
    function link(dependent, prerequisite, kind) {
      dependent.addPrerequisite(prerequisite).kind = kind;
    }
    link(review, spec, DependencyKind.FinishStart);
    link(approved, review, DependencyKind.FinishStart);
    link(impl, approved, DependencyKind.FinishStart);
    link(docs, impl, DependencyKind.StartStart);
    link(test, impl, DependencyKind.FinishFinish);
    link(test, docs, DependencyKind.StartFinish);
    link(release, test, DependencyKind.FinishStart);
    var engineer = scenario.rootResource.addMember();
    engineer.name = '李工程师';
    impl.addAssignment(engineer);
    spec.note = '明确发布范围与接口。';
    docs.manualStartDate = new Date(2026, 9, 12, 9, 0, 0);
  });
  return 'created';
})()
